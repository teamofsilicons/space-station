-- Preserve all records and definitions in their existing physical namespaces. New accounts
-- receive their immutable UUID as a namespace. The former tos namespace belongs to si:tos.
CREATE TABLE account_owners (
  uuid uuid PRIMARY KEY,
  namespace text NOT NULL UNIQUE,
  actor text NOT NULL,
  kind text NOT NULL CHECK (kind IN ('carbon', 'silicon')),
  active boolean NOT NULL DEFAULT true,
  updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO account_owners (uuid, namespace, actor, kind)
VALUES ('0080c488-a9e9-4c22-aa91-1c7a639f1d7b', 'tos', 'si:tos', 'silicon');

CREATE TABLE account_sessions (
  id_hash text PRIMARY KEY,
  account_uuid uuid NOT NULL REFERENCES account_owners(uuid),
  actor text NOT NULL,
  kind text NOT NULL,
  access_token_enc text NOT NULL,
  refresh_token_enc text NOT NULL,
  access_expires_at timestamptz NOT NULL,
  expires_at timestamptz NOT NULL,
  checked_at timestamptz NOT NULL DEFAULT now(),
  browser_group text,
  secret_enc text NOT NULL,
  world text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON account_sessions (account_uuid);
CREATE INDEX ON account_sessions (browser_group);
CREATE TABLE account_login_receipts (
  login_hash text PRIMARY KEY,
  id_hash text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE account_handoffs (
  code_hash text PRIMARY KEY,
  state_hash text NOT NULL,
  secret_enc text NOT NULL,
  expires_at timestamptz NOT NULL
);
CREATE TABLE account_events (event_id uuid PRIMARY KEY, received_at timestamptz NOT NULL DEFAULT now());

ALTER TABLE api_keys ADD COLUMN owner_uuid uuid REFERENCES account_owners(uuid);
UPDATE api_keys SET owner_uuid='0080c488-a9e9-4c22-aa91-1c7a639f1d7b'
WHERE org='tos';
