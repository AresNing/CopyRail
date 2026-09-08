-- CloudKit change pages do not guarantee that referenced entities arrive
-- first. Retain the latest accepted relation without creating placeholder
-- clips or boards, and commit it together with the private change token.
CREATE TABLE IF NOT EXISTS sync_deferred_memberships (
    entity_id TEXT PRIMARY KEY NOT NULL,
    pinboard_id TEXT NOT NULL,
    clip_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS sync_deferred_memberships_dependencies_idx
    ON sync_deferred_memberships(pinboard_id, clip_id);
CREATE INDEX IF NOT EXISTS sync_deferred_memberships_clip_idx
    ON sync_deferred_memberships(clip_id);

PRAGMA user_version = 14;
