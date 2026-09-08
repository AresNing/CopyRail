CREATE TABLE IF NOT EXISTS sync_conflicts (
    id TEXT PRIMARY KEY NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    local_change_json TEXT NOT NULL,
    remote_envelope_json TEXT NOT NULL,
    remote_would_win INTEGER NOT NULL CHECK (remote_would_win IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    resolved_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sync_conflicts_unresolved
ON sync_conflicts(resolved_at_ms, created_at_ms DESC);

PRAGMA user_version = 9;
