CREATE TABLE IF NOT EXISTS mcp_clients (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL,
    token_hash BLOB UNIQUE NOT NULL,
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER,
    revoked_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS mcp_clients_active_idx
    ON mcp_clients(revoked_at_ms, created_at_ms DESC);

PRAGMA user_version = 7;
