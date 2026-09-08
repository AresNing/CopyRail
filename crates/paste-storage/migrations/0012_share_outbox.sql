CREATE TABLE IF NOT EXISTS cloudkit_share_outbox (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL REFERENCES sync_outbox(operation_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('pending', 'acknowledged')),
    created_at_ms INTEGER NOT NULL,
    acknowledged_at_ms INTEGER,
    PRIMARY KEY (share_id, operation_id)
);

CREATE INDEX IF NOT EXISTS idx_cloudkit_share_outbox_pending
ON cloudkit_share_outbox(share_id, state, created_at_ms, operation_id);

PRAGMA user_version = 12;
