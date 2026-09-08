PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS clips (
    id TEXT PRIMARY KEY NOT NULL,
    captured_at_ms INTEGER NOT NULL,
    last_copied_at_ms INTEGER NOT NULL,
    source_bundle_id TEXT NOT NULL,
    source_display_name TEXT NOT NULL,
    device_id TEXT NOT NULL,
    device_display_name TEXT NOT NULL,
    content_kind TEXT NOT NULL,
    title TEXT NOT NULL,
    searchable_text TEXT NOT NULL,
    content_hash BLOB NOT NULL,
    deleted_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS clips_last_copied_idx
    ON clips(last_copied_at_ms DESC)
    WHERE deleted_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS clips_content_hash_idx
    ON clips(content_hash)
    WHERE deleted_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS clips_source_idx
    ON clips(source_bundle_id, last_copied_at_ms DESC)
    WHERE deleted_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS clips_device_idx
    ON clips(device_id, last_copied_at_ms DESC)
    WHERE deleted_at_ms IS NULL;

CREATE TABLE IF NOT EXISTS blobs (
    content_hash BLOB PRIMARY KEY NOT NULL,
    byte_len INTEGER NOT NULL,
    bytes BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS representations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    representation_kind TEXT NOT NULL,
    mime_type TEXT,
    file_name TEXT,
    byte_len INTEGER NOT NULL,
    content_hash BLOB NOT NULL REFERENCES blobs(content_hash),
    text_preview TEXT,
    UNIQUE(clip_id, ordinal)
);

CREATE INDEX IF NOT EXISTS representations_clip_idx ON representations(clip_id, ordinal);
CREATE INDEX IF NOT EXISTS representations_blob_idx ON representations(content_hash);

CREATE TABLE IF NOT EXISTS pinboards (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    sort_order INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    is_shared INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS pinboard_items (
    pinboard_id TEXT NOT NULL REFERENCES pinboards(id) ON DELETE CASCADE,
    clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(pinboard_id, clip_id)
);

CREATE INDEX IF NOT EXISTS pinboard_items_order_idx
    ON pinboard_items(pinboard_id, position, created_at_ms);
CREATE INDEX IF NOT EXISTS pinboard_items_clip_idx ON pinboard_items(clip_id);

CREATE VIRTUAL TABLE IF NOT EXISTS clip_search USING fts5(
    clip_id UNINDEXED,
    title,
    body,
    app_name,
    device_name,
    tokenize = 'unicode61 remove_diacritics 2'
);

PRAGMA user_version = 1;
