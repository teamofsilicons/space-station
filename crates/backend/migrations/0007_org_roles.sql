-- IAM's org role is part of the authorization snapshot. Keep it with the directory mirror so
-- org owners and admins can inspect every table, regardless of each table's access list.
ALTER TABLE iam_members ADD COLUMN org_role text NOT NULL DEFAULT 'member';
