-- Every definition, stored once; records live in ClickHouse. `window_versions.window_id` is the
-- doc's `window` (a reserved word in Postgres).
CREATE TABLE tables (
  org text NOT NULL,
  id text NOT NULL,
  key_hash text NOT NULL UNIQUE,
  access jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  key_rotated_at timestamptz,
  PRIMARY KEY (org, id)
);

CREATE TABLE windows (
  id uuid PRIMARY KEY,
  org text NOT NULL,
  name text NOT NULL,
  access jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  current_version uuid,
  state jsonb,
  state_version text,
  produced_at timestamptz
);
CREATE INDEX ON windows (org);

CREATE TABLE window_versions (
  id uuid PRIMARY KEY,
  window_id uuid NOT NULL REFERENCES windows (id) ON DELETE CASCADE,
  name text NOT NULL,
  processor text NOT NULL,
  renderer text NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE windows ADD FOREIGN KEY (current_version) REFERENCES window_versions (id);

CREATE TABLE access_tokens (
  org text NOT NULL,
  actor text NOT NULL,
  token_hash text NOT NULL UNIQUE,
  token_enc text NOT NULL,
  identity_snapshot jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  last_used_at timestamptz,
  PRIMARY KEY (org, actor)
);

CREATE TABLE notifications (
  id uuid PRIMARY KEY,
  org text NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  enabled boolean NOT NULL DEFAULT true,
  recipients jsonb NOT NULL DEFAULT '[]',
  cursors jsonb NOT NULL DEFAULT '{}',
  last_cron_at timestamptz,
  current_version uuid
);
CREATE INDEX ON notifications (org);

CREATE TABLE notification_versions (
  id uuid PRIMARY KEY,
  notification uuid NOT NULL REFERENCES notifications (id) ON DELETE CASCADE,
  def jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE notifications ADD FOREIGN KEY (current_version) REFERENCES notification_versions (id);

CREATE TABLE notification_events (
  id bigserial PRIMARY KEY,
  notification uuid NOT NULL REFERENCES notifications (id) ON DELETE CASCADE,
  org text NOT NULL,
  dedup_key text NOT NULL,
  text text NOT NULL,
  metadata jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON notification_events (notification, dedup_key, created_at DESC);

CREATE TABLE webhooks (
  id uuid PRIMARY KEY,
  org text NOT NULL,
  url text NOT NULL,
  secret_enc text NOT NULL,
  actor text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON webhooks (org);
CREATE UNIQUE INDEX ON webhooks (org, actor) WHERE actor IS NOT NULL;

CREATE TABLE api_keys (
  id uuid PRIMARY KEY,
  org text NOT NULL,
  key_hash text NOT NULL UNIQUE,
  scopes text[] NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  last_used_at timestamptz
);
CREATE INDEX ON api_keys (org);

CREATE TABLE sessions (
  id_hash text PRIMARY KEY,
  actor text NOT NULL,
  actor_kind text NOT NULL,
  oat_enc text NOT NULL,
  ort_enc text NOT NULL,
  expires_at timestamptz NOT NULL,
  refresh_key uuid,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON sessions (actor);

CREATE TABLE dev_errors (
  id bigserial PRIMARY KEY,
  org text NOT NULL,
  source text NOT NULL,
  ref text NOT NULL,
  message text NOT NULL,
  detail jsonb NOT NULL DEFAULT '{}',
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON dev_errors (org, created_at DESC);

CREATE TABLE iam_events (
  event_id uuid PRIMARY KEY,
  received_at timestamptz NOT NULL DEFAULT now()
);
