CREATE TABLE IF NOT EXISTS sync_conflict_blobs (
    conflict_id TEXT NOT NULL REFERENCES sync_conflicts(id) ON DELETE CASCADE,
    content_hash BLOB NOT NULL,
    byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
    bytes BLOB NOT NULL,
    PRIMARY KEY (conflict_id, content_hash)
);

PRAGMA user_version = 10;
