#!/usr/bin/env python3
"""Creates one isolated PostgreSQL database; never reads the configured application database."""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("migration", ROOT / "scripts/migrate-public-identifiers.py")
migration = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migration)

ORG_UUID = "10000000-0000-0000-0000-000000000001"
PRINCIPAL_UUID = "20000000-0000-0000-0000-000000000001"
MEMBERSHIP_UUID = "30000000-0000-0000-0000-000000000001"
WINDOW_UUID = "40000000-0000-0000-0000-000000000001"
VERSION_UUID = "50000000-0000-0000-0000-000000000001"
NOTIFICATION_UUID = "60000000-0000-0000-0000-000000000001"
WEBHOOK_UUID = "70000000-0000-0000-0000-000000000001"
# AES-256-GCM, SS_KEY=[7;32], empty AAD, plaintext "spacewindow-preserved".
CIPHERTEXT = "AQEBAQEBAQEBAQEBBZHo1PXIhWC1u6sMAEUVQs2JYpF8eBjKzmZrmYzLPvIhwOfLbw=="


def inventory(scope=""):
    identities = [
        {"actor_type": "carbon", "org_id": None, "old_id": "alice0", "new_id": "c:alice0"},
        {"actor_type": "silicon", "org_id": "tos", "old_id": "bot:tos", "new_id": "si:bot"},
        {"actor_type": "application", "org_id": "tos", "old_id": "tos>spacestation", "new_id": "spacestation"},
    ]
    for row in identities:
        row["scope_key"] = scope
    members = [{**row, "org_id": "tos", "org_uuid": ORG_UUID, "principal_id": None, "membership_id": None}
               for row in identities if row["actor_type"] != "application"]
    members[0].update(principal_id=PRINCIPAL_UUID, membership_id=MEMBERSHIP_UUID)
    return {"scope_key": scope, "identities": identities, "memberships": members}


def sql(text, expected=None):
    result = migration.run_psql(text, env)
    if expected is None:
        assert result.returncode == 0, result.stderr
    else:
        assert result.returncode != 0 and expected in result.stderr, result.stderr
    return result.stdout.strip()


def snapshot():
    tables = ["tables", "windows", "window_versions", "access_tokens", "notifications", "notification_versions",
              "notification_events", "webhooks", "api_keys", "sessions", "iam_members", "iam_events", "dev_errors"]
    query = "SELECT jsonb_build_object(" + ",".join(
        f"'{table}', (SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]') FROM {table} t)"
        for table in tables) + ")::text;"
    result = subprocess.run(["psql", "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", query],
                            env=env, capture_output=True, text=True, check=True)
    return json.loads(result.stdout)


def seed():
    sql("""TRUNCATE tables, windows, window_versions, access_tokens, notifications, notification_versions,
      notification_events, webhooks, api_keys, sessions, iam_members, iam_events, dev_errors CASCADE;
      UPDATE public_identifier_migration SET ready=false, environment_bound=false, scope_key=NULL, testing_key_sha256=NULL,
        mapping_sha256=NULL, migrated_at=NULL;
    """)
    sql(f"""
    INSERT INTO iam_members (org, actor, membership_id, kind, status, version, principal_id, org_uuid)
      VALUES ('tos','alice0','{MEMBERSHIP_UUID}','carbon','active',7,'{PRINCIPAL_UUID}','{ORG_UUID}'),
             ('tos','bot:tos','bot:tos[tos]','silicon','removed',8,'bot:tos','{ORG_UUID}');
    INSERT INTO access_tokens (org,actor,token_hash,token_enc,membership_id,principal_id)
      VALUES ('tos','alice0','access-hash','{CIPHERTEXT}','{MEMBERSHIP_UUID}','{PRINCIPAL_UUID}');
    INSERT INTO tables (org,id,key_hash,access,created_by)
      VALUES ('tos','logs','table-hash','["@alice0","@bot:tos","tag:alice0"]','alice0');
    INSERT INTO windows (id,org,name,access,created_by,state,state_version)
      VALUES ('{WINDOW_UUID}','tos','alice0 is plain text','["@alice0"]','bot:tos','{{"actor":"alice0"}}','alice0');
    INSERT INTO window_versions (id,window_id,name,processor,renderer,created_by)
      VALUES ('{VERSION_UUID}','{WINDOW_UUID}','v1','alice0','bot:tos','alice0');
    UPDATE windows SET current_version='{VERSION_UUID}';
    INSERT INTO notifications (id,org,created_by,recipients,cursors)
      VALUES ('{NOTIFICATION_UUID}','tos','alice0','["@alice0","@bot:tos","webhook:{WEBHOOK_UUID}"]','{{"logs":77}}');
    INSERT INTO notification_versions (id,notification,created_by,def)
      VALUES ('{VERSION_UUID}','{NOTIFICATION_UUID}','bot:tos',
        '{{"access":["@alice0","@bot:tos"],"description":"@alice0","sql":"SELECT ''alice0''"}}');
    UPDATE notifications SET current_version='{VERSION_UUID}';
    INSERT INTO notification_events (notification,org,dedup_key,text,metadata)
      VALUES ('{NOTIFICATION_UUID}','tos','alice0','@alice0','{{"actor":"bot:tos"}}');
    INSERT INTO webhooks (id,org,url,secret_enc,actor,created_by)
      VALUES ('{WEBHOOK_UUID}','tos','https://example.com/bot:tos','exact-encrypted-secret','bot:tos','alice0');
    INSERT INTO api_keys (id,org,key_hash,scopes,created_by)
      VALUES ('{WEBHOOK_UUID}','tos','api-hash',ARRAY['tables'],'alice0');
    INSERT INTO sessions (id_hash,actor,kind,org,membership_id,oat_enc,ort_enc,expires_at)
      VALUES ('session-hash','alice0','carbon','tos','{MEMBERSHIP_UUID}','exact-oat','exact-ort',now()+interval '1h');
    INSERT INTO iam_events (event_id) VALUES ('{WEBHOOK_UUID}');
    INSERT INTO dev_errors (org,source,ref,message,detail)
      VALUES ('tos','notification','alice0','@alice0','{{"actor":"bot:tos"}}');
    """)


def check_preserved(before, after):
    expected = copy.deepcopy(before)
    actor = {"alice0": "c:alice0", "bot:tos": "si:bot"}
    for table, rows in expected.items():
        for row in rows:
            if "created_by" in row:
                row["created_by"] = actor[row["created_by"]]
            if table in ("iam_members", "access_tokens", "webhooks") and row.get("actor"):
                row["actor"] = actor[row["actor"]]
            if table in ("iam_members", "access_tokens"):
                if row.get("membership_id") == MEMBERSHIP_UUID:
                    row["membership_id"] = "c:alice0[tos]"
                if row.get("membership_id") == "bot:tos[tos]":
                    row["membership_id"] = "si:bot[tos]"
                if row.get("principal_id") == "bot:tos":
                    row["principal_id"] = "si:bot"
            for key in ("access", "recipients"):
                if key in row:
                    row[key] = ["@" + actor[x[1:]] if x.startswith("@") else x for x in row[key]]
            if table == "notification_versions":
                row["def"]["access"] = ["@" + actor[x[1:]] for x in row["def"]["access"]]
    expected["sessions"] = []
    assert expected == after, "migration changed non-identity data, encrypted bytes, hashes or resource keys"
    ciphertext = after["access_tokens"][0]["token_enc"]
    subprocess.run(["node", "-e", """
      const c = require('node:crypto'), b = Buffer.from(process.argv[1], 'base64');
      const d = c.createDecipheriv('aes-256-gcm', Buffer.alloc(32,7), b.subarray(0,12));
      d.setAuthTag(b.subarray(-16));
      const plain = Buffer.concat([d.update(b.subarray(12,-16)),d.final()]).toString();
      require('node:assert/strict').equal(plain, 'spacewindow-preserved');
    """, ciphertext], check=True)


def check_cli(redis_url):
    """Opt-in wrapper check against a dedicated empty Redis database, retaining every guard."""
    def redis(*args):
        return subprocess.run(["redis-cli", "-u", redis_url, "--raw", *args],
                              capture_output=True, text=True, check=True).stdout.strip()

    assert redis("DBSIZE") == "0", "CLI check requires a dedicated empty Redis database"
    seed()
    before = snapshot()
    cli_env = {**env, "DATABASE_URL": f"postgresql:///{db}", "REDIS_URL": redis_url}
    cli_env.pop("SILICON_IAM_TEST_KEY", None)
    with tempfile.TemporaryDirectory(prefix="ss-migration-cli-") as directory:
        mapping = Path(directory) / "mapping.json"
        mapping.write_text(json.dumps(inventory()))
        backup = Path(directory) / "before.dump"

        def run(*args):
            return subprocess.run([sys.executable, str(ROOT / "scripts/migrate-public-identifiers.py"),
                                   str(mapping), "--scope-key", "", "--offline", *args],
                                  capture_output=True, text=True, env=cli_env)

        for key, command in (("lease:engine", "SET"), ("staging", "RPUSH"), ("flushing", "SET")):
            redis(command, key, "test-pending-work")
            try:
                result = run("--apply", "--backup", str(backup))
                assert result.returncode != 0 and "staging/flushing work remains" in result.stderr, result.stderr
                assert not backup.exists() and snapshot() == before
                assert redis("EXISTS", key) == "1", "guard must never discard pending work"
            finally:
                redis("DEL", key)
        result = run()
        assert result.returncode == 0 and '"preview"' in result.stdout, result.stderr
        assert snapshot() == before and not backup.exists()
        result = run("--apply", "--backup", str(backup))
        assert result.returncode == 0 and '"apply"' in result.stdout, result.stderr
        assert backup.stat().st_mode & 0o777 == 0o600 and backup.stat().st_size > 0
        subprocess.run(["pg_restore", "--list", str(backup)], check=True, stdout=subprocess.DEVNULL)
        check_preserved(before, snapshot())
        saved, migrated = backup.read_bytes(), snapshot()
        result = run("--apply", "--backup", str(backup))
        assert result.returncode != 0 and "File exists" in result.stderr, result.stderr
        assert backup.read_bytes() == saved and snapshot() == migrated, "existing backup must not be overwritten"
    assert redis("DBSIZE") == "0"
    print("PASS: CLI Redis guards, preview/apply, mode-0600 readable backup and overwrite refusal")


if __name__ == "__main__":
    # libpq settings can target a disposable local test server, never DATABASE_URL.
    db = "ss_identifier_test_" + uuid.uuid4().hex
    env = {**os.environ, "PGDATABASE": db, "PGAPPNAME": "ss-identifier-test"}
    subprocess.run(["createdb", "--", db], env=env, check=True)
    try:
        sql("CREATE TABLE _sqlx_migrations (version bigint PRIMARY KEY, description text NOT NULL, "
            "installed_on timestamptz NOT NULL DEFAULT now(), success boolean NOT NULL, "
            "checksum bytea NOT NULL, execution_time bigint NOT NULL);")
        for path in sorted((ROOT / "crates/backend/migrations").glob("*.sql")):
            version, description = path.stem.split("_", 1)
            sql(path.read_text() + f"\nINSERT INTO _sqlx_migrations (version,description,success,checksum,execution_time) "
                f"VALUES ({int(version)},'{description.replace('_', ' ')}',true,"
                f"decode('{hashlib.sha384(path.read_bytes()).hexdigest()}', 'hex'),0);")
        for scope, key in (("", None), ("testing:one", "A" * 32), ("testing:two", "B" * 32)):
            seed()
            # A newly bootstrapped store is fenced by its configured key before any export exists.
            sql("UPDATE public_identifier_migration SET environment_bound=true, testing_key_sha256='wrong';")
            sql(migration.transaction(inventory(scope), scope, key, True), "fence mismatch")
            sql("UPDATE public_identifier_migration SET environment_bound=false, testing_key_sha256=NULL;")
            if not scope:
                sql("DROP TABLE public_identifier_migration; DELETE FROM _sqlx_migrations WHERE version=8;")
            before = snapshot()
            mapping = inventory(scope)
            sql(migration.transaction(mapping, scope, key))
            assert snapshot() == before, "preview changed the database"
            sql("UPDATE sessions SET refresh_key='80000000-0000-0000-0000-000000000001';")
            pending = snapshot()
            sql(migration.transaction(mapping, scope, key, True), "pending IAM refresh")
            assert snapshot() == pending, "pending refresh refusal was not atomic"
            sql("UPDATE sessions SET refresh_key=NULL;")
            sql("UPDATE notifications SET recipients=recipients || '\"@missing\"'::jsonb;")
            unmapped = snapshot()
            sql(migration.transaction(mapping, scope, key, True), "unmapped or ambiguous actor")
            assert snapshot() == unmapped, "late unmapped recipient did not roll back earlier identity updates"
            sql("UPDATE notifications SET recipients=recipients - '@missing';")
            sql("UPDATE iam_members SET kind='carbon' WHERE actor='bot:tos';")
            wrong_kind = snapshot()
            sql(migration.transaction(mapping, scope, key, True), "actor kind mismatch")
            assert snapshot() == wrong_kind
            sql("UPDATE iam_members SET kind='silicon' WHERE actor='bot:tos';")
            sql("UPDATE iam_members SET principal_id='90000000-0000-0000-0000-000000000001' WHERE kind='carbon';")
            mismatch = snapshot()
            sql(migration.transaction(mapping, scope, key, True), "unverified principal_id")
            assert snapshot() == mismatch
            sql(f"UPDATE iam_members SET principal_id='{PRINCIPAL_UUID}' WHERE kind='carbon';")
            wrong_org = copy.deepcopy(mapping)
            wrong_org["memberships"][0]["org_id"] = "other"
            wrong_org["memberships"][0]["org_uuid"] = "10000000-0000-0000-0000-000000000002"
            sql(migration.transaction(wrong_org, scope, key, True), "IAM organization UUID mismatch")
            assert snapshot() == before
            sql("INSERT INTO iam_members SELECT org,'c:alice0',membership_id,kind,status,tags,org_name,version,"
                "updated_at,principal_id,org_uuid,org_role FROM iam_members WHERE kind='carbon';")
            collision = snapshot()
            sql(migration.transaction(mapping, scope, key, True), "colliding local actor")
            assert snapshot() == collision
            sql("DELETE FROM iam_members WHERE actor='c:alice0';")
            sql(migration.transaction(mapping, scope, key, True))
            after = snapshot()
            check_preserved(before, after)
            sql(migration.transaction(mapping, scope, key, True), "already migrated")
            assert snapshot() == after
            sql(migration.transaction(inventory("another"), "another", "C" * 32, True), "fence mismatch")
            assert snapshot() == after
            sql(migration.transaction(inventory("test:'$$"), "test:'$$", "C" * 32, True), "fence mismatch")
            assert snapshot() == after, "world names must never escape SQL literals"
        bad = inventory()
        overlapping = inventory()
        overlapping["identities"].append({"scope_key":"", "actor_type":"carbon", "org_id":None,
                                         "old_id":"spacestation", "new_id":"c:spacestation"})
        migration.validate(overlapping, "")
        for corrupt in (
            {**inventory(), "scope_key": "foreign"},
            {**inventory(), "identities": [{"scope_key":"", "actor_type": []}]},
        ):
            try:
                migration.validate(corrupt, "")
                raise AssertionError("bad mapping accepted")
            except ValueError:
                pass
        bad["identities"].append({"scope_key":"", "actor_type":"silicon", "org_id":"other",
                                  "old_id":"bot:other", "new_id":"si:bot"})
        try:
            migration.validate(bad, "")
            raise AssertionError("global collision accepted")
        except ValueError as error:
            assert "colliding" in str(error)
        if redis_url := os.environ.get("SS_IDENTIFIER_TEST_REDIS_URL"):
            check_cli(redis_url)
        print("PASS: production/two test worlds, preview, exact data/ciphertext/UUID preservation, session policy, "
              "pending refresh, collision, ownership/principal mismatch, world fencing and rollback")
    finally:
        subprocess.run(["dropdb", "--", db], env=env, check=True)
