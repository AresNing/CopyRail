ALTER TABLE sync_entity_versions ADD COLUMN change_json TEXT;

UPDATE sync_entity_versions
SET change_json = (
    SELECT o.change_json
    FROM sync_outbox o
    WHERE o.entity_kind = sync_entity_versions.entity_kind
      AND o.entity_id = sync_entity_versions.entity_id
    ORDER BY o.created_at_ms DESC, o.logical_counter DESC, o.operation_id DESC
    LIMIT 1
);

PRAGMA user_version = 8;
