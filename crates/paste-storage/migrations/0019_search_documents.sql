-- Derived, compact live-search metadata. Explicit INTEGER PRIMARY KEY keeps
-- the FTS document identity stable across VACUUM; public clip UUIDs do not change.
CREATE TABLE IF NOT EXISTS clip_search_documents (
    doc_id INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE REFERENCES clips(id) ON DELETE CASCADE,
    last_copied_at_ms INTEGER NOT NULL,
    content_kind TEXT NOT NULL,
    source_bundle_id TEXT NOT NULL,
    device_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS search_documents_recent_idx
    ON clip_search_documents(last_copied_at_ms DESC, id DESC);
CREATE INDEX IF NOT EXISTS search_documents_source_idx
    ON clip_search_documents(source_bundle_id, last_copied_at_ms DESC, id DESC);
CREATE INDEX IF NOT EXISTS search_documents_device_idx
    ON clip_search_documents(device_id, last_copied_at_ms DESC, id DESC);

-- Rebuild only derived state in the migration transaction. Keep all original
-- bytes, soft-deleted clips, boards, outbox UUIDs and immutable snapshots intact.
DELETE FROM clip_search;
DELETE FROM clip_search_documents;
INSERT INTO clip_search_documents(id, last_copied_at_ms, content_kind, source_bundle_id, device_id)
    SELECT id, last_copied_at_ms, content_kind, source_bundle_id, device_id
    FROM clips WHERE deleted_at_ms IS NULL ORDER BY last_copied_at_ms, id;
INSERT INTO clip_search(rowid, clip_id, title, body, app_name, device_name)
    SELECT d.doc_id, c.id, c.title, c.searchable_text, c.source_display_name, c.device_display_name
    FROM clip_search_documents d JOIN clips c ON c.id = d.id;

-- Every local/remote write uses clips as the authority. These triggers live in
-- the same transaction, so rollback cannot leave metadata and FTS out of sync.
CREATE TRIGGER IF NOT EXISTS clips_search_insert AFTER INSERT ON clips
WHEN new.deleted_at_ms IS NULL BEGIN
    INSERT INTO clip_search_documents(id, last_copied_at_ms, content_kind, source_bundle_id, device_id)
        VALUES(new.id, new.last_copied_at_ms, new.content_kind, new.source_bundle_id, new.device_id);
    INSERT INTO clip_search(rowid, clip_id, title, body, app_name, device_name)
        SELECT doc_id, new.id, new.title, new.searchable_text, new.source_display_name, new.device_display_name
        FROM clip_search_documents WHERE id = new.id;
END;

CREATE TRIGGER IF NOT EXISTS clips_search_remove BEFORE DELETE ON clips BEGIN
    DELETE FROM clip_search WHERE rowid = (SELECT doc_id FROM clip_search_documents WHERE id = old.id);
    DELETE FROM clip_search_documents WHERE id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS clips_search_soft_delete AFTER UPDATE OF deleted_at_ms ON clips
WHEN old.deleted_at_ms IS NULL AND new.deleted_at_ms IS NOT NULL BEGIN
    DELETE FROM clip_search WHERE rowid = (SELECT doc_id FROM clip_search_documents WHERE id = old.id);
    DELETE FROM clip_search_documents WHERE id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS clips_search_restore AFTER UPDATE OF deleted_at_ms ON clips
WHEN old.deleted_at_ms IS NOT NULL AND new.deleted_at_ms IS NULL BEGIN
    INSERT INTO clip_search_documents(id, last_copied_at_ms, content_kind, source_bundle_id, device_id)
        VALUES(new.id, new.last_copied_at_ms, new.content_kind, new.source_bundle_id, new.device_id);
    INSERT INTO clip_search(rowid, clip_id, title, body, app_name, device_name)
        SELECT doc_id, new.id, new.title, new.searchable_text, new.source_display_name, new.device_display_name
        FROM clip_search_documents WHERE id = new.id;
END;

CREATE TRIGGER IF NOT EXISTS clips_search_metadata
AFTER UPDATE OF last_copied_at_ms, content_kind, source_bundle_id, device_id ON clips
WHEN old.deleted_at_ms IS NULL AND new.deleted_at_ms IS NULL BEGIN
    UPDATE clip_search_documents SET last_copied_at_ms = new.last_copied_at_ms,
        content_kind = new.content_kind, source_bundle_id = new.source_bundle_id, device_id = new.device_id
        WHERE id = new.id;
END;

CREATE TRIGGER IF NOT EXISTS clips_search_text
AFTER UPDATE OF title, searchable_text, source_display_name, device_display_name ON clips
WHEN old.deleted_at_ms IS NULL AND new.deleted_at_ms IS NULL AND (
    old.title IS NOT new.title OR old.searchable_text IS NOT new.searchable_text OR
    old.source_display_name IS NOT new.source_display_name OR old.device_display_name IS NOT new.device_display_name
) BEGIN
    DELETE FROM clip_search WHERE rowid = (SELECT doc_id FROM clip_search_documents WHERE id = old.id);
    INSERT INTO clip_search(rowid, clip_id, title, body, app_name, device_name)
        SELECT doc_id, new.id, new.title, new.searchable_text, new.source_display_name, new.device_display_name
        FROM clip_search_documents WHERE id = new.id;
END;

PRAGMA user_version = 19;
