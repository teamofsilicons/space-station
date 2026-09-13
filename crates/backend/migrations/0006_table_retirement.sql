ALTER TABLE tables ADD COLUMN retired_at timestamptz;
CREATE INDEX tables_org_retired ON tables (org, retired_at);
