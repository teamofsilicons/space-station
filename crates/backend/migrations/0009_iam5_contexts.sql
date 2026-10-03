-- Existing IAM 4 rows cannot prove a single IAM 5 organization. They remain inaccessible.
ALTER TABLE sessions ADD COLUMN iam_contract smallint NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN browser_group text;
ALTER TABLE sessions ADD COLUMN world text NOT NULL DEFAULT 'legacy';
ALTER TABLE sessions ADD COLUMN browser_secret_enc text;
CREATE INDEX sessions_browser_contexts ON sessions (browser_group) WHERE iam_contract = 5;
-- Keep receipts after logout so replaying a spent login cannot recreate its session.
CREATE TABLE iam_login_receipts (
  request_key uuid PRIMARY KEY,
  id_hash text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
