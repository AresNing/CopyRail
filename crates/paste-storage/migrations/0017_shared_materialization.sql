-- Executed inside the migration transaction, including legacy wire freezing.
CREATE TABLE IF NOT EXISTS shared_clip_bindings (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    remote_clip_id TEXT NOT NULL,
    local_clip_id TEXT NOT NULL,
    private_origin INTEGER NOT NULL CHECK(private_origin IN (0, 1)),
    PRIMARY KEY(share_id, remote_clip_id),
    UNIQUE(share_id, local_clip_id)
);
CREATE INDEX IF NOT EXISTS idx_shared_clip_local ON shared_clip_bindings(local_clip_id);
CREATE TABLE IF NOT EXISTS shared_entity_states (
    share_id TEXT NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    PRIMARY KEY(share_id, entity_kind, entity_id),
    FOREIGN KEY(share_id, operation_id) REFERENCES cloudkit_share_inbox(share_id, operation_id)
);
CREATE TABLE IF NOT EXISTS shared_operation_receipts (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    envelope_hash BLOB NOT NULL CHECK(length(envelope_hash) = 32),
    PRIMARY KEY(share_id, operation_id)
);
CREATE TABLE IF NOT EXISTS shared_conflicts (
    share_id TEXT NOT NULL,
    local_operation_id TEXT NOT NULL,
    remote_operation_id TEXT NOT NULL,
    PRIMARY KEY(share_id, local_operation_id, remote_operation_id),
    FOREIGN KEY(share_id, local_operation_id) REFERENCES cloudkit_share_inbox(share_id, operation_id),
    FOREIGN KEY(share_id, remote_operation_id) REFERENCES cloudkit_share_inbox(share_id, operation_id)
);
CREATE TABLE IF NOT EXISTS shared_projection_queue (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    remote_clip_id TEXT NOT NULL,
    PRIMARY KEY(share_id, remote_clip_id)
);
-- Local IDs are never uploaded to a foreign zone. Translation is immutable at
-- enqueue, not at retry time. The existing snapshot retains attachment bytes.
CREATE TABLE IF NOT EXISTS shared_wire_outbox (
    share_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    PRIMARY KEY(share_id, operation_id),
    FOREIGN KEY(share_id, operation_id) REFERENCES cloudkit_share_outbox(share_id, operation_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS shared_only_outbox (
    operation_id TEXT PRIMARY KEY REFERENCES sync_outbox(operation_id) ON DELETE CASCADE
);
PRAGMA user_version = 17;
