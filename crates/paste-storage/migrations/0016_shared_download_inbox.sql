BEGIN IMMEDIATE;

-- Remote identities live only inside the registered share. In particular,
-- neither a participant's UUID nor a blob hash can select private history.
CREATE TABLE IF NOT EXISTS cloudkit_share_inbox (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    received_at_ms INTEGER NOT NULL,
    PRIMARY KEY (share_id, operation_id)
);
CREATE INDEX IF NOT EXISTS idx_share_inbox_received
ON cloudkit_share_inbox(share_id, received_at_ms, operation_id);

CREATE TABLE IF NOT EXISTS cloudkit_share_inbox_blobs (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
    byte_len INTEGER NOT NULL CHECK(byte_len >= 0),
    bytes BLOB NOT NULL CHECK(length(bytes) = byte_len),
    PRIMARY KEY (share_id, content_hash)
);
CREATE TABLE IF NOT EXISTS cloudkit_share_inbox_blob_refs (
    share_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    content_hash BLOB NOT NULL,
    PRIMARY KEY (share_id, operation_id, content_hash),
    FOREIGN KEY (share_id, operation_id)
      REFERENCES cloudkit_share_inbox(share_id, operation_id) ON DELETE CASCADE,
    FOREIGN KEY (share_id, content_hash)
      REFERENCES cloudkit_share_inbox_blobs(share_id, content_hash)
);

-- Earlier versions could store a shared token without a durable receive
-- journal. Replay those zones once; preserve all outbox UUIDs and private tokens.
UPDATE cloudkit_shares SET server_change_token = NULL WHERE state != 'revoked';

PRAGMA user_version = 16;
COMMIT;
