CREATE TABLE IF NOT EXISTS cloudkit_shares (
    id TEXT PRIMARY KEY NOT NULL,
    pinboard_id TEXT UNIQUE REFERENCES pinboards(id) ON DELETE CASCADE,
    zone_name TEXT NOT NULL,
    owner_name TEXT,
    share_record_name TEXT,
    share_url TEXT,
    role TEXT NOT NULL CHECK (role IN ('owner', 'participant')),
    permission TEXT NOT NULL CHECK (permission IN ('read_only', 'read_write')),
    state TEXT NOT NULL CHECK (state IN ('preparing', 'active', 'revoked')),
    server_change_token BLOB,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(owner_name, zone_name)
);

CREATE INDEX IF NOT EXISTS idx_cloudkit_shares_state
ON cloudkit_shares(state, role, updated_at_ms);

PRAGMA user_version = 11;
