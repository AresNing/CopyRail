-- A change UUID always identifies the same payload, even across retries,
-- intervening edits, soft deletion and independent share acknowledgements.
CREATE TABLE IF NOT EXISTS sync_outbox_snapshots (
    operation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES sync_outbox(operation_id) ON DELETE CASCADE,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_outbox_blob_refs (
    operation_id TEXT NOT NULL
        REFERENCES sync_outbox_snapshots(operation_id) ON DELETE CASCADE,
    content_hash BLOB NOT NULL REFERENCES blobs(content_hash),
    PRIMARY KEY(operation_id, content_hash)
);
CREATE INDEX IF NOT EXISTS sync_outbox_blob_hash_idx
    ON sync_outbox_blob_refs(content_hash);

-- The Rust migration renews legacy pending changes with fresh UUIDs in the
-- same transaction; their original payloads cannot be reconstructed safely.
PRAGMA user_version = 13;
