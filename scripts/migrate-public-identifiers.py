#!/usr/bin/env python3
"""Offline IAM-authoritative PostgreSQL conversion. Preview rolls back the actual transaction."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = {
    "carbon": r"c:[a-z0-9_-]{3,30}",
    "silicon": r"si:[a-z0-9_-]{3,50}",
    "application": r"[a-z][a-z0-9_-]{0,79}",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def is_uuid(value):
    try:
        return isinstance(value, str) and str(uuid.UUID(value)) == value
    except ValueError:
        return False


def validate(mapping, scope_key):
    """Check the complete world export, including identities absent from this consumer."""
    require(isinstance(mapping, dict) and mapping.get("scope_key") == scope_key, "mapping scope_key mismatch")
    identities, memberships = mapping.get("identities"), mapping.get("memberships")
    require(isinstance(identities, list) and isinstance(memberships, list), "identities and memberships must be arrays")
    old, new, aliases = {}, {}, {}
    for row in identities:
        require(isinstance(row, dict) and row.get("scope_key") == scope_key, "identity world mismatch")
        kind, owner, source, target = (row.get(k) for k in ("actor_type", "org_id", "old_id", "new_id"))
        require(isinstance(kind, str) and kind in PATTERNS and isinstance(source, str)
                and isinstance(target, str), "invalid identity mapping")
        require(re.fullmatch(PATTERNS[kind], target), "invalid canonical identifier")
        if kind == "carbon":
            require(owner is None, "Carbon ownership comes from memberships; identity org_id must be null")
            legacy = target.removeprefix("c:")
        else:
            require(isinstance(owner, str) and re.fullmatch(r"[a-z0-9_-]{3,50}", owner), "invalid owning organization")
            legacy = f"{target.removeprefix('si:')}:{owner}" if kind == "silicon" else f"{owner}>{target}"
        require(source in (target, legacy), "mapping is not the verified identifier schema conversion")
        require((kind, source) not in old and (kind, target) not in new, "duplicate or colliding identity mapping")
        old[kind, source], new[kind, target] = row, row
        for value in {source, target}:
            alias = ("application" if kind == "application" else "actor", value)
            require(alias not in aliases, "ambiguous old/new identity mapping")
            aliases[alias] = row
    seen, private_principals, private_memberships, orgs, actor_principals = set(), {}, {}, {}, {}
    for row in memberships:
        require(isinstance(row, dict) and row.get("scope_key") == scope_key, "membership world mismatch")
        kind, org, source, target = (row.get(k) for k in ("actor_type", "org_id", "old_id", "new_id"))
        require(kind in ("carbon", "silicon") and isinstance(source, str) and isinstance(target, str)
                and isinstance(org, str)
                and re.fullmatch(r"[a-z0-9_-]{3,50}", org), "invalid membership")
        identity = old.get((kind, source))
        require(identity is not None and identity["new_id"] == target, "membership missing authoritative identity")
        require(kind != "silicon" or identity["org_id"] == org, "Silicon membership ownership mismatch")
        require((org, target) not in seen, "duplicate membership")
        seen.add((org, target))
        require(is_uuid(row.get("org_uuid")), "membership needs IAM organization UUID")
        require(org not in orgs or orgs[org] == row["org_uuid"], "conflicting IAM organization UUID")
        require(row["org_uuid"] not in orgs.values() or orgs.get(org) == row["org_uuid"],
                "IAM organization UUID has multiple owners")
        orgs[org] = row["org_uuid"]
        for field, used, key in (("principal_id", private_principals, target),
                                 ("membership_id", private_memberships, (org, target))):
            value = row.get(field)
            require(value is None or is_uuid(value), f"{field} must be an unchanged IAM UUID or null for public IDs")
            if value is not None:
                require(value not in used or used[value] == key, f"conflicting IAM {field} binding")
                used[value] = key
                if field == "principal_id":
                    require(target not in actor_principals or actor_principals[target] == value,
                            "actor has conflicting IAM principal UUIDs")
                    actor_principals[target] = value
    return mapping


def literal(value):
    return "NULL" if value is None else "'" + str(value).replace("'", "''") + "'"


def transaction(mapping, scope_key, testing_key, apply=False):
    validate(mapping, scope_key)
    require(bool(scope_key) == bool(testing_key), "test scope requires SILICON_IAM_TEST_KEY; production forbids it")
    encoded = json.dumps(mapping, sort_keys=True, separators=(",", ":"))
    digest = hashlib.sha256(encoded.encode()).hexdigest()
    key_digest = hashlib.sha256(testing_key.encode()).hexdigest() if testing_key else None
    migrations = sorted((ROOT / "crates/backend/migrations").glob("*.sql"))
    schema = ROOT / "crates/backend/migrations/0008_public_identifiers.sql"
    prefix = ["BEGIN; SET LOCAL lock_timeout = '5s'; SET LOCAL statement_timeout = '5min';",
              "SET LOCAL standard_conforming_strings = on; SET LOCAL search_path = public, pg_temp;",
              "LOCK TABLE _sqlx_migrations IN EXCLUSIVE MODE;"]
    # Refuse an unexpected schema, rather than omit a newly introduced identity column.
    for path in migrations:
        version = int(path.stem.split("_", 1)[0])
        if version >= 8:
            continue
        checksum = hashlib.sha384(path.read_bytes()).hexdigest()
        prefix.append(f"""DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM _sqlx_migrations
          WHERE version = {version} AND success AND encode(checksum, 'hex') = '{checksum}')
          THEN RAISE EXCEPTION 'schema migration {version} missing or changed'; END IF; END $$;""")
    prefix.append(r"""DO $$ BEGIN
      IF EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version > 8 OR NOT success)
      THEN RAISE EXCEPTION 'unsupported schema migration'; END IF;
      IF EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database()
          AND pid <> pg_backend_pid() AND backend_type = 'client backend')
      THEN RAISE EXCEPTION 'other database clients remain: stop all writers first'; END IF;
      END $$;
      SELECT NOT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 8) AS install_schema \gset
      \if :install_schema
    """)
    prefix.append(schema.read_text())
    prefix.append(fr"""INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
      VALUES (8, 'public identifiers', true, decode('{hashlib.sha384(schema.read_bytes()).hexdigest()}', 'hex'), 0);
      \endif
      LOCK TABLE public_identifier_migration, tables, windows, window_versions, access_tokens,
        notifications, notification_versions, notification_events, webhooks, api_keys, sessions,
        iam_members, iam_events, dev_errors IN ACCESS EXCLUSIVE MODE;
      CREATE TEMP TABLE migration_input (body jsonb) ON COMMIT DROP;
      INSERT INTO migration_input VALUES ({literal(encoded)}::jsonb);
      CREATE TEMP TABLE member_map ON COMMIT DROP AS
      SELECT * FROM jsonb_to_recordset((SELECT body->'memberships' FROM migration_input)) AS m(
        scope_key text, actor_type text, org_id text, old_id text, new_id text,
        org_uuid text, principal_id text, membership_id text);
      DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 8 AND success
            AND encode(checksum, 'hex') = '{hashlib.sha384(schema.read_bytes()).hexdigest()}')
        THEN RAISE EXCEPTION 'schema migration 8 checksum mismatch'; END IF;
        IF EXISTS (SELECT 1 FROM public_identifier_migration WHERE
          environment_bound AND (testing_key_sha256 IS DISTINCT FROM {literal(key_digest)}
            OR (scope_key IS NOT NULL AND scope_key IS DISTINCT FROM
              (SELECT body->>'scope_key' FROM migration_input))))
        THEN RAISE EXCEPTION 'stored IAM world/testing key fence mismatch'; END IF;
        IF EXISTS (SELECT 1 FROM public_identifier_migration WHERE migrated_at IS NOT NULL)
        THEN RAISE EXCEPTION 'already migrated; do not invalidate post-cutover sessions or remap identities'; END IF;
        IF EXISTS (SELECT 1 FROM sessions WHERE refresh_key IS NOT NULL)
        THEN RAISE EXCEPTION 'pending IAM refresh: reconcile using original key before cutover'; END IF;
      END $$;
    """)
    prefix.append(SQL)
    prefix.append(f"""UPDATE public_identifier_migration SET ready = true, environment_bound = true,
      scope_key = {literal(scope_key)},
      testing_key_sha256 = {literal(key_digest)}, mapping_sha256 = '{digest}', migrated_at = now();
      SELECT json_build_object('mode', '{'apply' if apply else 'preview'}', 'scope_key', {literal(scope_key)},
        'mapping_sha256', '{digest}', 'rows', (SELECT json_object_agg(name, count) FROM before_counts));
      {'COMMIT' if apply else 'ROLLBACK'};""")
    return "\n".join(prefix)


SQL = r"""
CREATE FUNCTION pg_temp.mapped_actor(org text, actor text, kind text DEFAULT NULL) RETURNS text
LANGUAGE plpgsql AS $$ DECLARE m member_map%ROWTYPE; BEGIN
  SELECT * INTO STRICT m FROM member_map
    WHERE org_id = org AND actor IN (old_id, new_id);
  IF kind IS NOT NULL AND kind IS DISTINCT FROM m.actor_type THEN
    RAISE EXCEPTION 'actor kind mismatch'; END IF;
  RETURN m.new_id;
EXCEPTION WHEN NO_DATA_FOUND OR TOO_MANY_ROWS THEN
  RAISE EXCEPTION 'unmapped or ambiguous actor in organization %: %', org, actor;
END $$;
CREATE FUNCTION pg_temp.mapped_reference(org text, actor text, value text, field text) RETURNS text
LANGUAGE plpgsql AS $$ DECLARE m member_map%ROWTYPE; BEGIN
  IF value IS NULL THEN RETURN NULL; END IF;
  PERFORM pg_temp.mapped_actor(org, actor);
  SELECT * INTO STRICT m FROM member_map WHERE org_id = org AND actor IN (old_id, new_id);
  IF field = 'membership_id' THEN
    IF value IN (m.old_id || '[' || org || ']', m.new_id || '[' || org || ']') THEN
      RETURN m.new_id || '[' || org || ']'; END IF;
    IF value = m.membership_id THEN RETURN m.new_id || '[' || org || ']'; END IF;
  ELSIF field = 'principal_id' THEN
    IF value IN (m.old_id, m.new_id) THEN RETURN m.new_id; END IF;
    IF value = m.principal_id THEN RETURN value; END IF;
  END IF;
  RAISE EXCEPTION 'unverified % binding for actor in organization %', field, org;
END $$;
CREATE FUNCTION pg_temp.mapped_list(org text, entries jsonb) RETURNS jsonb
LANGUAGE plpgsql AS $$ DECLARE item jsonb; text_value text; result jsonb := '[]'; BEGIN
  IF jsonb_typeof(entries) IS DISTINCT FROM 'array' THEN RAISE EXCEPTION 'invalid identity list'; END IF;
  FOR item IN SELECT * FROM jsonb_array_elements(entries) LOOP
    IF jsonb_typeof(item) <> 'string' THEN RAISE EXCEPTION 'non-string identity list entry'; END IF;
    text_value := item #>> '{}';
    IF left(text_value, 1) = '@' THEN
      text_value := '@' || pg_temp.mapped_actor(org, substring(text_value FROM 2)); END IF;
    result := result || jsonb_build_array(text_value);
  END LOOP;
  RETURN result;
END $$;
CREATE TEMP TABLE before_counts (name text, count bigint) ON COMMIT DROP;
INSERT INTO before_counts
  SELECT 'tables', count(*) FROM tables UNION ALL SELECT 'windows', count(*) FROM windows
  UNION ALL SELECT 'window_versions', count(*) FROM window_versions
  UNION ALL SELECT 'access_tokens', count(*) FROM access_tokens
  UNION ALL SELECT 'notifications', count(*) FROM notifications
  UNION ALL SELECT 'notification_versions', count(*) FROM notification_versions
  UNION ALL SELECT 'notification_events', count(*) FROM notification_events
  UNION ALL SELECT 'webhooks', count(*) FROM webhooks UNION ALL SELECT 'api_keys', count(*) FROM api_keys
  UNION ALL SELECT 'sessions_invalidated', count(*) FROM sessions
  UNION ALL SELECT 'iam_members', count(*) FROM iam_members;
-- Verify principal, membership and organization bindings before changing any row. This covers
-- canonical records too; syntactic validity is not evidence that they belong to this IAM world.
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM iam_members m WHERE m.org_uuid IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM member_map i WHERE i.org_id = m.org AND m.actor IN (i.old_id, i.new_id)
        AND i.org_uuid = m.org_uuid)) THEN RAISE EXCEPTION 'IAM organization UUID mismatch'; END IF;
  IF EXISTS (SELECT 1 FROM iam_members GROUP BY org, pg_temp.mapped_actor(org, actor) HAVING count(*) > 1)
    OR EXISTS (SELECT 1 FROM access_tokens GROUP BY org, pg_temp.mapped_actor(org, actor) HAVING count(*) > 1)
    OR EXISTS (SELECT 1 FROM webhooks WHERE actor IS NOT NULL
      GROUP BY org, pg_temp.mapped_actor(org, actor) HAVING count(*) > 1)
    THEN RAISE EXCEPTION 'colliding local actor bindings'; END IF;
END $$;
-- Sessions are validated before invalidation, and unresolved refreshes were rejected above.
SELECT count(pg_temp.mapped_actor(org, actor, kind)),
  count(pg_temp.mapped_reference(org, actor, membership_id, 'membership_id')) FROM sessions;
UPDATE iam_members SET actor = pg_temp.mapped_actor(org, actor, kind),
  membership_id = pg_temp.mapped_reference(org, actor, membership_id, 'membership_id'),
  principal_id = pg_temp.mapped_reference(org, actor, principal_id, 'principal_id');
UPDATE access_tokens SET actor = pg_temp.mapped_actor(org, actor),
  membership_id = pg_temp.mapped_reference(org, actor, membership_id, 'membership_id'),
  principal_id = pg_temp.mapped_reference(org, actor, principal_id, 'principal_id');
UPDATE tables SET created_by = pg_temp.mapped_actor(org, created_by), access = pg_temp.mapped_list(org, access);
UPDATE windows SET created_by = pg_temp.mapped_actor(org, created_by), access = pg_temp.mapped_list(org, access);
UPDATE window_versions v SET created_by = pg_temp.mapped_actor(w.org, v.created_by)
  FROM windows w WHERE w.id = v.window_id;
UPDATE notifications SET created_by = pg_temp.mapped_actor(org, created_by),
  recipients = pg_temp.mapped_list(org, recipients);
UPDATE notification_versions v SET created_by = pg_temp.mapped_actor(n.org, v.created_by),
  def = CASE WHEN v.def ? 'access' THEN jsonb_set(v.def, '{access}', pg_temp.mapped_list(n.org, v.def->'access'))
    ELSE v.def END FROM notifications n WHERE n.id = v.notification;
UPDATE webhooks SET created_by = pg_temp.mapped_actor(org, created_by),
  actor = CASE WHEN actor IS NULL THEN NULL ELSE pg_temp.mapped_actor(org, actor, 'silicon') END;
UPDATE api_keys SET created_by = pg_temp.mapped_actor(org, created_by);
DELETE FROM sessions;
"""


def run_psql(sql, env=None, database_url=None):
    connection = ["--dbname", database_url] if database_url else []
    return subprocess.run(["psql", *connection, "-X", "--no-password", "-q", "-v", "ON_ERROR_STOP=1", "-f", "-"],
                          input=sql, text=True, capture_output=True, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mapping", type=Path)
    parser.add_argument("--scope-key", required=True, help="exact IAM world; empty string for production")
    parser.add_argument("--offline", action="store_true", help="attest writers stopped and notification sends reconciled")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--backup", type=Path, help="new private pg_dump archive, required with --apply")
    args = parser.parse_args()
    require(args.offline, "stop writers, drain/reconcile notification sends, then pass --offline")
    require(os.environ.get("DATABASE_URL") and os.environ.get("REDIS_URL"), "DATABASE_URL and REDIS_URL required")
    require(not args.apply or args.backup, "--apply requires a new --backup path")
    mapping = json.loads(args.mapping.read_text())
    sql = transaction(mapping, args.scope_key, os.environ.get("SILICON_IAM_TEST_KEY") or None, args.apply)
    redis = subprocess.run(["redis-cli", "--no-auth-warning", "-u", os.environ["REDIS_URL"], "--raw", "EVAL",
                            "return {redis.call('EXISTS','lease:engine'),redis.call('LLEN','staging'),"
                            "redis.call('EXISTS','flushing')}", "0"], capture_output=True, text=True)
    require(redis.returncode == 0 and redis.stdout.split() == ["0", "0", "0"],
            "Redis unavailable, engine still running, or staging/flushing work remains; drain without deleting work")
    env = {**os.environ, "PGAPPNAME": "space-station-public-id-migration"}
    if args.apply:
        with os.fdopen(os.open(args.backup, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), "wb") as backup:
            result = subprocess.run(["pg_dump", "--dbname", os.environ["DATABASE_URL"], "--no-password", "--format=custom"], stdout=backup,
                                    stderr=subprocess.PIPE, env=env)
        require(result.returncode == 0, "pg_dump failed; incomplete backup retained; no migration attempted")
        result = subprocess.run(["pg_restore", "--list", str(args.backup)], stdout=subprocess.DEVNULL,
                                stderr=subprocess.PIPE)
        require(result.returncode == 0, "backup archive is unreadable; no migration attempted")
    result = run_psql(sql, env, os.environ["DATABASE_URL"])
    require(result.returncode == 0, "migration rolled back: " + result.stderr.strip())
    print(result.stdout.strip())


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, json.JSONDecodeError) as error:
        sys.exit(str(error))
