-- Existing stores must pass the IAM-authoritative offline conversion before the upgraded
-- service can connect to IAM or start its workers. Empty stores can bootstrap normally.
CREATE TABLE public_identifier_migration (
  singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
  ready boolean NOT NULL,
  environment_bound boolean NOT NULL DEFAULT false,
  scope_key text,
  testing_key_sha256 text,
  mapping_sha256 text,
  migrated_at timestamptz,
  CHECK (environment_bound OR (scope_key IS NULL AND testing_key_sha256 IS NULL)),
  CHECK (scope_key IS NULL
      OR (scope_key = '' AND testing_key_sha256 IS NULL)
      OR (scope_key <> '' AND testing_key_sha256 IS NOT NULL))
);
INSERT INTO public_identifier_migration (ready)
SELECT NOT EXISTS (
  SELECT 1 FROM iam_members UNION ALL SELECT 1 FROM access_tokens UNION ALL SELECT 1 FROM sessions
  UNION ALL SELECT 1 FROM tables UNION ALL SELECT 1 FROM windows UNION ALL SELECT 1 FROM notifications
  UNION ALL SELECT 1 FROM webhooks UNION ALL SELECT 1 FROM api_keys
);
