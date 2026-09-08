BEGIN IMMEDIATE;
ALTER TABLE representations ADD COLUMN native_type TEXT;
ALTER TABLE sync_outbox ADD COLUMN envelope_schema_version INTEGER NOT NULL DEFAULT 1;
PRAGMA user_version = 15;
COMMIT;
