-- A callback state can spend one SLT. Identical retries retain their original IAM mutation.
CREATE TABLE iam_login_attempts (
  state_hash text PRIMARY KEY,
  identity_kind text NOT NULL CHECK (identity_kind IN ('carbon', 'silicon')),
  slt_hash text,
  expires_at timestamptz NOT NULL
);
CREATE INDEX iam_login_attempts_expiry ON iam_login_attempts (expires_at);
