ALTER TABLE clips ADD COLUMN delete_batch_id TEXT;

CREATE INDEX IF NOT EXISTS idx_clips_delete_batch
    ON clips(delete_batch_id, deleted_at_ms);

PRAGMA user_version = 4;
