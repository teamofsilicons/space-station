-- Identity on IAM's short-lived-token contract: a session is bound to one org and re-proved by
-- introspection, names and tags reach us only through webhooks into the directory mirror, and an
-- access token reads its tags from that mirror rather than a snapshot taken at resolve time. The
-- sessions that exist were minted by a login flow the service no longer has, so they go.
DELETE FROM sessions;
ALTER TABLE sessions RENAME COLUMN actor_kind TO kind;
ALTER TABLE sessions
  ADD COLUMN org text NOT NULL,
  ADD COLUMN membership_id text,
  ADD COLUMN checked_at timestamptz NOT NULL DEFAULT now();

ALTER TABLE access_tokens DROP COLUMN identity_snapshot;

-- The directory mirror: one row per (org, actor), written only by the IAM webhook receiver from
-- `data.current.members[]`, kept at the highest membership version seen. `org_name` rides along
-- so the org list can show a name without a directory read, which an Application may not make.
CREATE TABLE iam_members (
  org text NOT NULL,
  actor text NOT NULL,
  membership_id text NOT NULL,
  kind text NOT NULL,
  status text NOT NULL,
  tags jsonb NOT NULL DEFAULT '[]',
  display_name text,
  org_name text,
  version bigint NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (org, actor)
);
CREATE INDEX ON iam_members (actor);
