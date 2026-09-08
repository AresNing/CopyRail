CREATE TABLE IF NOT EXISTS sync_clocks (
    device_id TEXT PRIMARY KEY NOT NULL,
    wall_time_ms INTEGER NOT NULL,
    counter INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_entity_versions (
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    version_json TEXT NOT NULL,
    PRIMARY KEY(entity_kind, entity_id)
);

CREATE TABLE IF NOT EXISTS sync_outbox (
    operation_id TEXT PRIMARY KEY NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    change_kind TEXT NOT NULL,
    change_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending', 'acknowledged')),
    created_at_ms INTEGER NOT NULL,
    logical_counter INTEGER NOT NULL,
    node_id TEXT NOT NULL,
    acknowledged_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS sync_outbox_pending_idx
    ON sync_outbox(state, created_at_ms, logical_counter, node_id, operation_id);
CREATE INDEX IF NOT EXISTS sync_outbox_entity_idx
    ON sync_outbox(entity_kind, entity_id, created_at_ms);

CREATE TABLE IF NOT EXISTS sync_tokens (
    scope TEXT PRIMARY KEY NOT NULL,
    token BLOB NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

PRAGMA user_version = 6;
