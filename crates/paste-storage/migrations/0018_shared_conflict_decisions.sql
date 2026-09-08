BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS shared_conflict_decisions (
    share_id TEXT NOT NULL REFERENCES cloudkit_shares(id) ON DELETE CASCADE,
    first_operation_id TEXT NOT NULL,
    second_operation_id TEXT NOT NULL,
    resolution TEXT NOT NULL CHECK(resolution IN ('keep_current', 'use_first', 'use_second')),
    resolved_operation_id TEXT NOT NULL,
    resolved_at_ms INTEGER NOT NULL,
    PRIMARY KEY(share_id, first_operation_id, second_operation_id)
);
PRAGMA user_version = 18;
COMMIT;
