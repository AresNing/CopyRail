CREATE TABLE IF NOT EXISTS preview_cache (
    clip_id TEXT PRIMARY KEY NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    source_content_hash BLOB NOT NULL,
    media_type TEXT NOT NULL,
    bytes BLOB NOT NULL,
    pixel_width INTEGER NOT NULL,
    pixel_height INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
);

PRAGMA user_version = 3;
