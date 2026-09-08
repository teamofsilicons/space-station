-- The ids a removal tombstone names — its membership, its principal, and on the envelope the org's
-- uuid — next to what we already hold, so a tombstone whose membership id matches nothing is still
-- resolved to an actor inside the right org: the mirror keeps the principal and the org uuid, an
-- access token the principal.
ALTER TABLE iam_members ADD COLUMN principal_id text, ADD COLUMN org_uuid text;
ALTER TABLE access_tokens ADD COLUMN principal_id text;
