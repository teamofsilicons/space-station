-- A window's versions are addressed by name, in the app's picker and in `space-station windows
-- publish --name`, so two versions of one window may not share one.
CREATE UNIQUE INDEX window_versions_window_name ON window_versions (window_id, name);
