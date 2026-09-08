-- Adopting silicon-iam-client 1.2.1 and closing the removal-tombstone hole.
--   * `sessions.tags`: the tags from this session's own `authorization` snapshot, written at login
--     and every re-proof, so a session's Identity reads its proven tags before the shared mirror.
--     NULL means no snapshot has disclosed tags yet (fall back to the mirror).
--   * `access_tokens.membership_id`: the membership a `spacewindow-` token belongs to, captured at
--     mint from the mirror, so a removal tombstone (which names only a membership) can revoke the
--     token even when its actor has no live session.
--   * `iam_members.display_name` was written by the receiver and read by nobody — the handle is the
--     name everywhere — so it goes.
ALTER TABLE sessions ADD COLUMN tags jsonb;
ALTER TABLE access_tokens ADD COLUMN membership_id text;
ALTER TABLE iam_members DROP COLUMN display_name;
