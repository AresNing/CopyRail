#![forbid(unsafe_code)]

mod action_context;
mod share_inbox;
mod share_materialize;
pub use action_context::{ClipActionBoard, ClipActionContext, ClipActionItem};
pub use share_inbox::{SharedInboxBatch, SharedInboxReport};
pub use share_materialize::SharedMaterializationReport;
pub use share_materialize::{
    SharedConflictId, SharedConflictResolution, SharedConflictSummary, SharedConflictVersion,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    str::FromStr,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use paste_domain::{
    CaptureFlags, CapturePreferences, CapturedItem, CapturedRepresentation, ClipId, ClipItem,
    ContentKind, DesktopPreferences, DeviceFacet, DeviceId, DeviceMetadata,
    PersistedRepresentation, Pinboard, PinboardId, RepresentationKind, RetentionPolicy,
    SearchFacets, SearchHit, SearchPage, SearchQuery, SourceApplication, SourceFacet,
};
use paste_sync::{
    HybridClock, HybridTimestamp, PinboardMembershipSnapshot, PreparedSyncBatch,
    PreparedSyncOperation, RemoteConflict, RemoteDisposition, SyncBlob, SyncChange, SyncChangeKind,
    SyncEntity, SyncEntityKind, SyncEnvelope, SyncPayload, SyncScope, VersionVector,
    coalesce_changes, decide_remote_change,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, backup::Backup, params,
    params_from_iter, types::Value,
};
use thiserror::Error;

const MIGRATION_V1: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_device_settings.sql");
const MIGRATION_V3: &str = include_str!("../migrations/0003_preview_cache.sql");
const MIGRATION_V4: &str = include_str!("../migrations/0004_delete_batches.sql");
const MIGRATION_V5: &str = include_str!("../migrations/0005_backup_identity.sql");
const MIGRATION_V6: &str = include_str!("../migrations/0006_sync_outbox.sql");
const MIGRATION_V7: &str = include_str!("../migrations/0007_mcp_access.sql");
const MIGRATION_V8: &str = include_str!("../migrations/0008_sync_change_metadata.sql");
const MIGRATION_V9: &str = include_str!("../migrations/0009_sync_conflicts.sql");
const MIGRATION_V10: &str = include_str!("../migrations/0010_sync_conflict_blobs.sql");
const MIGRATION_V11: &str = include_str!("../migrations/0011_cloudkit_shares.sql");
const MIGRATION_V12: &str = include_str!("../migrations/0012_share_outbox.sql");
const MIGRATION_V13: &str = include_str!("../migrations/0013_immutable_sync_snapshots.sql");
const MIGRATION_V14: &str = include_str!("../migrations/0014_deferred_sync_memberships.sql");
const MIGRATION_V15: &str = include_str!("../migrations/0015_native_representation_types.sql");
const MIGRATION_V16: &str = include_str!("../migrations/0016_shared_download_inbox.sql");
const MIGRATION_V17: &str = include_str!("../migrations/0017_shared_materialization.sql");
const MIGRATION_V18: &str = include_str!("../migrations/0018_shared_conflict_decisions.sql");
const MIGRATION_V19: &str = include_str!("../migrations/0019_search_documents.sql");
const APPLICATION_ID: u32 = 1_347_572_564;
const SCHEMA_VERSION: u32 = 19;
const MIN_BACKUP_SCHEMA_VERSION: u32 = 5;
const TITLE_LIMIT: usize = 80;
const TEXT_PREVIEW_LIMIT: usize = 512;
const TEXTUAL_EDIT_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const IMAGE_EDIT_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const REMOTE_SYNC_BLOB_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const REMOTE_SYNC_BATCH_BLOB_LIMIT: usize = 2_048;
const DEFERRED_MEMBERSHIP_LIMIT: usize = 100_000;
const DEFERRED_MEMBERSHIP_DRAIN_LIMIT: usize = 250;

pub struct SqliteStore {
    connection: Mutex<Connection>,
}

/// Item metadata and bytes read under one lock, so an editor never receives
/// the revision of one snapshot and the payload of a later sync update.
#[derive(Clone, Debug)]
pub struct RichTextEditSnapshot {
    pub item: ClipItem,
    pub representations: Vec<CapturedRepresentation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedPreview {
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpClient {
    pub id: uuid::Uuid,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteApplyReport {
    pub applied: usize,
    pub ignored: usize,
    /// Relations still waiting for referenced clips/boards after this batch.
    pub deferred: usize,
    pub conflicts: Vec<RemoteConflict>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncConflictSummary {
    pub id: uuid::Uuid,
    pub clip_id: ClipId,
    pub local_title: String,
    pub remote_title: String,
    pub remote_would_win: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncConflictResolution {
    KeepLocal,
    AcceptRemote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinboardShareRole {
    Owner,
    Participant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinboardSharePermission {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinboardShareState {
    Preparing,
    Active,
    Revoked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinboardShare {
    pub id: uuid::Uuid,
    pub pinboard_id: Option<PinboardId>,
    pub zone_name: String,
    pub owner_name: Option<String>,
    pub share_record_name: Option<String>,
    pub share_url: Option<String>,
    pub role: PinboardShareRole,
    pub permission: PinboardSharePermission,
    pub state: PinboardShareState,
    pub server_change_token: Option<Vec<u8>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection, false)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        Self::from_connection(connection, true)
    }

    pub fn export_backup(&self, path: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = path.as_ref();
        let parent = path.parent().ok_or(StorageError::InvalidBackupPath)?;
        fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(StorageError::InvalidBackupPath)?;
        let temporary = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

        let result = (|| {
            let source = self.lock()?;
            let mut destination = Connection::open(&temporary)?;
            let backup = Backup::new(&source, &mut destination)?;
            backup.run_to_completion(128, std::time::Duration::from_millis(5), None)?;
            drop(backup);
            ensure_integrity(&destination)?;
            drop(destination);
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn restore_backup(&self, path: impl AsRef<Path>) -> Result<(), StorageError> {
        let source = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        validate_backup_source(&source)?;

        let mut upgraded = Connection::open_in_memory()?;
        let source_backup = Backup::new(&source, &mut upgraded)?;
        source_backup.run_to_completion(128, std::time::Duration::from_millis(5), None)?;
        drop(source_backup);
        migrate(&mut upgraded)?;
        validate_current_database(&upgraded)?;

        let mut destination = self.lock()?;
        let backup = Backup::new(&upgraded, &mut destination)?;
        backup.run_to_completion(128, std::time::Duration::from_millis(5), None)?;
        drop(backup);
        destination.pragma_update(None, "foreign_keys", true)?;
        destination.pragma_update(None, "busy_timeout", 5_000_i64)?;
        ensure_integrity(&destination)
    }

    fn from_connection(mut connection: Connection, in_memory: bool) -> Result<Self, StorageError> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        if !in_memory {
            connection.pragma_update(None, "journal_mode", "WAL")?;
            connection.pragma_update(None, "synchronous", "NORMAL")?;
        }
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn insert_capture(&self, capture: &CapturedItem) -> Result<ClipItem, StorageError> {
        self.insert_captures(std::slice::from_ref(capture))?
            .pop()
            .ok_or(StorageError::EmptyBatch)
    }

    pub fn insert_captures(
        &self,
        captures: &[CapturedItem],
    ) -> Result<Vec<ClipItem>, StorageError> {
        self.insert_captures_transactional(captures, None)
    }

    /// Commit capture, search documents, outbox and retention as one unit.
    /// A failed cleanup must not make a committed capture look unsuccessful.
    pub fn insert_captures_with_retention(
        &self,
        captures: &[CapturedItem],
        policy: RetentionPolicy,
        now: DateTime<Utc>,
    ) -> Result<Vec<ClipItem>, StorageError> {
        policy.validate()?;
        self.insert_captures_transactional(captures, Some((policy, now)))
    }

    fn insert_captures_transactional(
        &self,
        captures: &[CapturedItem],
        retention: Option<(RetentionPolicy, DateTime<Utc>)>,
    ) -> Result<Vec<ClipItem>, StorageError> {
        for capture in captures {
            capture.validate()?;
            if !capture.flags.should_persist() {
                return Err(StorageError::CaptureRejected);
            }
        }
        if captures.is_empty() && retention.is_none() {
            return Ok(Vec::new());
        }

        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let items = captures
            .iter()
            .map(|capture| insert_capture_transaction(&transaction, capture))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some((policy, now)) = retention {
            apply_retention_transaction(&transaction, policy, now)?;
        }
        transaction.commit()?;
        Ok(items)
    }

    pub fn get_clip(&self, id: ClipId) -> Result<Option<ClipItem>, StorageError> {
        let connection = self.lock()?;
        match load_clip(&connection, &id.to_string()) {
            Ok(item) => Ok(Some(item)),
            Err(StorageError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn load_clipboard_payload(
        &self,
        id: ClipId,
    ) -> Result<Vec<CapturedRepresentation>, StorageError> {
        let connection = self.lock()?;
        if share_materialize::isolated_origin(&connection, &id.to_string())?.is_some() {
            ensure_exists(&connection, "clips", &id.to_string())?;
        }
        load_clipboard_payload_from_connection(&connection, &id.to_string())
    }

    pub fn list_history(&self, page: SearchPage) -> Result<Vec<ClipItem>, StorageError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id FROM clips
             WHERE deleted_at_ms IS NULL
             ORDER BY last_copied_at_ms DESC, id DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let ids = statement
            .query_map(params![page.limit, page.offset], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter().map(|id| load_clip(&connection, id)).collect()
    }

    pub fn history_position(&self, id: ClipId) -> Result<u32, StorageError> {
        let connection = self.lock()?;
        ensure_exists(&connection, "clips", &id.to_string())?;
        let position: i64 = connection.query_row(
            "SELECT COUNT(*) FROM clips newer
             WHERE newer.deleted_at_ms IS NULL AND (
               newer.last_copied_at_ms > (SELECT last_copied_at_ms FROM clips WHERE id = ?1)
               OR (newer.last_copied_at_ms = (SELECT last_copied_at_ms FROM clips WHERE id = ?1)
                   AND newer.id > ?1)
             )",
            params![id.to_string()],
            |row| row.get(0),
        )?;
        u32::try_from(position).map_err(|error| corrupt("history position", error))
    }

    pub fn list_search_facets(&self) -> Result<SearchFacets, StorageError> {
        let connection = self.lock()?;
        let mut source_statement = connection.prepare(
            "SELECT source_bundle_id, MAX(source_display_name), COUNT(*)
             FROM clips WHERE deleted_at_ms IS NULL
             GROUP BY source_bundle_id
             ORDER BY COUNT(*) DESC, MAX(source_display_name)",
        )?;
        let sources = source_statement
            .query_map([], |row| {
                Ok(SourceFacet {
                    bundle_identifier: row.get(0)?,
                    display_name: row.get(1)?,
                    item_count: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(source_statement);

        let mut device_statement = connection.prepare(
            "SELECT device_id, MAX(device_display_name), COUNT(*)
             FROM clips WHERE deleted_at_ms IS NULL
             GROUP BY device_id
             ORDER BY COUNT(*) DESC, MAX(device_display_name)",
        )?;
        let raw_devices = device_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let devices = raw_devices
            .into_iter()
            .map(|(id, display_name, item_count)| {
                Ok(DeviceFacet {
                    id: DeviceId::from_str(&id)
                        .map_err(|error| corrupt("facet device id", error))?,
                    display_name,
                    item_count,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(SearchFacets { sources, devices })
    }

    pub fn search(
        &self,
        query: &SearchQuery,
        page: SearchPage,
    ) -> Result<Vec<SearchHit>, StorageError> {
        self.search_internal(query, page, true)
    }

    /// Same complete match set, filters, recency and pagination as `search`,
    /// without calculating relevance metadata the timeline/MCP do not consume.
    pub fn search_items(
        &self,
        query: &SearchQuery,
        page: SearchPage,
    ) -> Result<Vec<ClipItem>, StorageError> {
        Ok(self
            .search_internal(query, page, false)?
            .into_iter()
            .map(|hit| hit.item)
            .collect())
    }

    fn search_internal(
        &self,
        query: &SearchQuery,
        page: SearchPage,
        include_rank: bool,
    ) -> Result<Vec<SearchHit>, StorageError> {
        let connection = self.lock()?;
        let has_text = !query.text.trim().is_empty();
        let ordered_board = !has_text && query.filters.pinboard_ids.len() == 1;
        let mut sql = if has_text && !include_rank {
            // Visit newer document IDs first to reduce top-N sort churn in the
            // common append-heavy workload. LIMIT -1 deliberately retains ALL
            // matches: recopy/import timestamps may disagree with document IDs.
            // Only the final, authoritative timestamp/UUID order is paginated.
            "SELECT c.id, 0.0 AS rank FROM (
                SELECT rowid AS doc_id FROM clip_search WHERE clip_search MATCH ?
                ORDER BY rowid DESC LIMIT -1
             ) matches CROSS JOIN clip_search_documents c ON c.doc_id = matches.doc_id"
                .to_owned()
        } else if has_text {
            "SELECT c.id, bm25(clip_search) AS rank
             FROM clip_search JOIN clip_search_documents c ON c.doc_id = clip_search.rowid"
                .to_owned()
        } else if ordered_board {
            // Start from this board's membership index instead of probing its
            // membership/position for every live item in the entire history.
            "SELECT c.id, 0.0 AS rank
             FROM pinboard_items pi JOIN clips c ON c.id = pi.clip_id"
                .to_owned()
        } else {
            "SELECT c.id, 0.0 AS rank FROM clips c".to_owned()
        };
        sql.push_str(if has_text {
            " WHERE 1 = 1"
        } else {
            " WHERE c.deleted_at_ms IS NULL"
        });
        let mut values = Vec::<Value>::new();

        if has_text {
            if include_rank {
                sql.push_str(" AND clip_search MATCH ?");
            }
            values.push(Value::Text(fts_expression(&query.text)));
        }
        append_in_filter(
            &mut sql,
            &mut values,
            "c.content_kind",
            query
                .filters
                .content_kinds
                .iter()
                .map(|kind| kind.as_str().to_owned()),
        );
        append_in_filter(
            &mut sql,
            &mut values,
            "c.source_bundle_id",
            query.filters.source_bundle_ids.iter().cloned(),
        );
        append_in_filter(
            &mut sql,
            &mut values,
            "c.device_id",
            query.filters.device_ids.iter().map(ToString::to_string),
        );
        if ordered_board {
            sql.push_str(" AND pi.pinboard_id = ?");
            values.push(Value::Text(query.filters.pinboard_ids[0].to_string()));
        } else if !query.filters.pinboard_ids.is_empty() {
            let placeholders = repeat_placeholders(query.filters.pinboard_ids.len());
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM pinboard_items pi \
                 WHERE pi.clip_id = c.id AND pi.pinboard_id IN ({placeholders}))"
            ));
            values.extend(
                query
                    .filters
                    .pinboard_ids
                    .iter()
                    .map(|id| Value::Text(id.to_string())),
            );
        }
        if let Some(after) = query.filters.copied_after {
            sql.push_str(" AND c.last_copied_at_ms >= ?");
            values.push(Value::Integer(after.timestamp_millis()));
        }
        if let Some(before) = query.filters.copied_before {
            sql.push_str(" AND c.last_copied_at_ms <= ?");
            values.push(Value::Integer(before.timestamp_millis()));
        }
        if ordered_board {
            sql.push_str(" ORDER BY pi.position ASC, c.last_copied_at_ms DESC, c.id DESC");
        } else {
            // Paste 6.3.11 changed text search to newest-first. BM25 remains
            // metadata, not an ordering criterion. Search the full match set
            // before applying pagination, including old and pinned results.
            sql.push_str(" ORDER BY c.last_copied_at_ms DESC, c.id DESC");
        }
        sql.push_str(" LIMIT ? OFFSET ?");
        values.push(Value::Integer(i64::from(page.limit)));
        values.push(Value::Integer(i64::from(page.offset)));

        let mut statement = connection.prepare_cached(&sql)?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, rank)| {
                Ok(SearchHit {
                    item: load_clip(&connection, &id)?,
                    rank,
                })
            })
            .collect()
    }

    pub fn rename_clip(&self, id: ClipId, title: &str) -> Result<(), StorageError> {
        let title = truncate_chars(title.trim(), TITLE_LIMIT);
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE clips SET title = ?1 WHERE id = ?2 AND deleted_at_ms IS NULL",
            params![title, id.to_string()],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Clip,
            &id.to_string(),
            SyncChangeKind::Save,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn create_textual_item(
        &self,
        kind: ContentKind,
        value: &str,
        source: SourceApplication,
        device: DeviceMetadata,
    ) -> Result<ClipItem, StorageError> {
        self.create_textual_item_in_pinboard(kind, value, source, device, None)
    }

    pub fn create_textual_item_in_pinboard(
        &self,
        kind: ContentKind,
        value: &str,
        source: SourceApplication,
        device: DeviceMetadata,
        pinboard_id: Option<PinboardId>,
    ) -> Result<ClipItem, StorageError> {
        let value = normalize_textual_edit(kind, value)?;
        let capture = CapturedItem {
            captured_at: Utc::now(),
            source,
            device,
            flags: CaptureFlags::default(),
            representations: textual_representations(kind, &value)?,
        };
        capture.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if let Some(pinboard_id) = pinboard_id {
            ensure_exists(&transaction, "pinboards", &pinboard_id.to_string())?;
            ensure_pinboard_share_writable(&transaction, pinboard_id)?;
        }
        let item = insert_capture_transaction_with_kind(&transaction, &capture, kind)?;
        if let Some(pinboard_id) = pinboard_id {
            pin_clips_transaction(&transaction, pinboard_id, &[item.id])?;
        }
        transaction.commit()?;
        Ok(item)
    }

    pub fn update_textual_clip(
        &self,
        id: ClipId,
        kind: ContentKind,
        title: &str,
        value: &str,
    ) -> Result<ClipItem, StorageError> {
        let value = normalize_textual_edit(kind, value)?;
        let representations = textual_representations(kind, &value)?;
        let aggregate_hash = aggregate_representation_hash(&representations);
        let title = if title.trim().is_empty() {
            suggested_title(&value, kind)
        } else {
            truncate_chars(title.trim(), TITLE_LIMIT)
        };
        let id = id.to_string();
        let now_ms = Utc::now().timestamp_millis();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE clips SET content_kind = ?1, title = ?2, searchable_text = ?3,
                    content_hash = ?4, last_copied_at_ms = ?5
             WHERE id = ?6 AND deleted_at_ms IS NULL",
            params![
                kind.as_str(),
                &title,
                &value,
                aggregate_hash.as_slice(),
                now_ms,
                &id,
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        transaction.execute(
            "DELETE FROM representations WHERE clip_id = ?1",
            params![&id],
        )?;
        transaction.execute("DELETE FROM preview_cache WHERE clip_id = ?1", params![&id])?;
        insert_representations(&transaction, &id, &representations, now_ms)?;
        delete_unreferenced_blobs(&transaction)?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Clip,
            &id,
            SyncChangeKind::Save,
        )?;
        let updated = load_clip(&transaction, &id)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn rich_text_edit_snapshot(
        &self,
        id: ClipId,
    ) -> Result<RichTextEditSnapshot, StorageError> {
        let connection = self.lock()?;
        let key = id.to_string();
        let item = load_clip(&connection, &key)?;
        if !rich_text_editable(&item) {
            return Err(StorageError::UnsupportedTextualEditKind);
        }
        ensure_clip_share_writable(&connection, &key)?;
        let representations = load_clipboard_payload_from_connection(&connection, &key)?;
        Ok(RichTextEditSnapshot {
            item,
            representations,
        })
    }

    /// Compare-and-save preserves the latest title/source/Pinboard metadata,
    /// but refuses to overwrite changed or deleted content behind the editor.
    pub fn update_rich_text_clip(
        &self,
        id: ClipId,
        expected_content_hash: [u8; 32],
        representations: &[CapturedRepresentation],
    ) -> Result<ClipItem, StorageError> {
        let mut bytes = 0usize;
        let mut native_types = BTreeSet::new();
        let mut text = None;
        let mut has_rtf = false;
        if representations.is_empty() || representations.len() > 4 {
            return Err(StorageError::InvalidRichTextEdit);
        }
        for representation in representations {
            representation.validate()?;
            bytes = bytes
                .checked_add(representation.bytes.len())
                .ok_or(StorageError::TextualEditTooLarge)?;
            if bytes > TEXTUAL_EDIT_LIMIT_BYTES || representation.bytes.is_empty() {
                return Err(StorageError::TextualEditTooLarge);
            }
            let native_type = representation
                .native_type
                .as_deref()
                .ok_or(StorageError::InvalidRichTextEdit)?;
            if !native_types.insert(native_type) {
                return Err(StorageError::InvalidRichTextEdit);
            }
            match (&representation.kind, native_type) {
                (RepresentationKind::PlainText, "public.utf8-plain-text") => {
                    text = representation.utf8_text();
                }
                (RepresentationKind::Rtf, "public.rtf")
                    if representation.bytes.starts_with(br"{\rtf") =>
                {
                    has_rtf = true;
                }
                (RepresentationKind::Html, "public.html") => {}
                (RepresentationKind::Custom(kind), "com.apple.flat-rtfd")
                    if kind == "com.apple.flat-rtfd" => {}
                _ => return Err(StorageError::InvalidRichTextEdit),
            }
        }
        let text = text
            .filter(|value| !value.trim().is_empty())
            .ok_or(StorageError::EmptyTextualEdit)?;
        if !has_rtf {
            return Err(StorageError::InvalidRichTextEdit);
        }
        let aggregate_hash = aggregate_representation_hash(representations);
        let key = id.to_string();
        let now_ms = Utc::now().timestamp_millis();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let current = load_clip(&transaction, &key)?;
        ensure_clip_share_writable(&transaction, &key)?;
        if current.content_hash != expected_content_hash {
            return Err(StorageError::StaleContentEdit);
        }
        if !rich_text_editable(&current) {
            return Err(StorageError::UnsupportedTextualEditKind);
        }
        if aggregate_hash == current.content_hash {
            return Ok(current);
        }
        transaction.execute(
            "UPDATE clips SET content_kind = 'rich_text', searchable_text = ?1, content_hash = ?2, last_copied_at_ms = ?3 WHERE id = ?4 AND deleted_at_ms IS NULL",
            params![text, aggregate_hash.as_slice(), now_ms, &key],
        )?;
        transaction.execute(
            "DELETE FROM representations WHERE clip_id = ?1",
            params![&key],
        )?;
        transaction.execute(
            "DELETE FROM preview_cache WHERE clip_id = ?1",
            params![&key],
        )?;
        insert_representations(&transaction, &key, representations, now_ms)?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Clip,
            &key,
            SyncChangeKind::Save,
        )?;
        delete_unreferenced_blobs(&transaction)?;
        let updated = load_clip(&transaction, &key)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn update_image_content(
        &self,
        id: ClipId,
        png_bytes: &[u8],
    ) -> Result<ClipItem, StorageError> {
        if png_bytes.len() > IMAGE_EDIT_LIMIT_BYTES || !png_bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(StorageError::InvalidImageEdit);
        }
        let representations = vec![CapturedRepresentation {
            native_type: None,
            kind: RepresentationKind::Png,
            mime_type: Some("image/png".into()),
            file_name: None,
            bytes: png_bytes.to_vec(),
        }];
        let aggregate_hash = aggregate_representation_hash(&representations);
        let id = id.to_string();
        let now_ms = Utc::now().timestamp_millis();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE clips SET content_hash = ?1, last_copied_at_ms = ?2
             WHERE id = ?3 AND content_kind = 'image' AND deleted_at_ms IS NULL",
            params![aggregate_hash.as_slice(), now_ms, &id],
        )?;
        if changed == 0 {
            return Err(StorageError::InvalidImageEdit);
        }
        transaction.execute(
            "DELETE FROM representations WHERE clip_id = ?1",
            params![&id],
        )?;
        transaction.execute("DELETE FROM preview_cache WHERE clip_id = ?1", params![&id])?;
        insert_representations(&transaction, &id, &representations, now_ms)?;
        delete_unreferenced_blobs(&transaction)?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Clip,
            &id,
            SyncChangeKind::Save,
        )?;
        let updated = load_clip(&transaction, &id)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn update_ocr_text(&self, id: ClipId, text: &str) -> Result<ClipItem, StorageError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(StorageError::EmptyOcrText);
        }
        if text.len() > TEXTUAL_EDIT_LIMIT_BYTES {
            return Err(StorageError::TextualEditTooLarge);
        }
        let id = id.to_string();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE clips SET searchable_text = ?1
             WHERE id = ?2 AND content_kind = 'image' AND deleted_at_ms IS NULL",
            params![text, &id],
        )?;
        if changed == 0 {
            return Err(StorageError::InvalidImageEdit);
        }
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Clip,
            &id,
            SyncChangeKind::Save,
        )?;
        let updated = load_clip(&transaction, &id)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn delete_clip(&self, id: ClipId, deleted_at: DateTime<Utc>) -> Result<(), StorageError> {
        self.delete_clips(&[id], deleted_at)
    }

    pub fn delete_clips(
        &self,
        ids: &[ClipId],
        deleted_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let batch_id = uuid::Uuid::new_v4().to_string();
        for id in ids {
            let id = id.to_string();
            let changed = transaction.execute(
                "UPDATE clips SET deleted_at_ms = ?1, delete_batch_id = ?2
                 WHERE id = ?3 AND deleted_at_ms IS NULL",
                params![deleted_at.timestamp_millis(), batch_id, id],
            )?;
            if changed == 0 {
                return Err(StorageError::NotFound);
            }
            enqueue_sync_change(
                &transaction,
                SyncEntityKind::Clip,
                &id,
                SyncChangeKind::Delete,
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn undo_last_delete(&self) -> Result<Option<ClipItem>, StorageError> {
        Ok(self.undo_last_delete_batch()?.into_iter().next())
    }

    pub fn undo_last_delete_batch(&self) -> Result<Vec<ClipItem>, StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let batch_id = transaction
            .query_row(
                "SELECT delete_batch_id FROM clips WHERE deleted_at_ms IS NOT NULL
                 ORDER BY deleted_at_ms DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(batch_id) = batch_id else {
            return Ok(Vec::new());
        };
        let mut statement = transaction.prepare(
            "SELECT id FROM clips WHERE delete_batch_id = ?1 AND deleted_at_ms IS NOT NULL
             ORDER BY last_copied_at_ms DESC",
        )?;
        let ids = statement
            .query_map(params![batch_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.execute(
            "UPDATE clips SET deleted_at_ms = NULL, delete_batch_id = NULL
             WHERE delete_batch_id = ?1",
            params![batch_id],
        )?;
        let mut restored = Vec::with_capacity(ids.len());
        for id in ids {
            enqueue_sync_change(
                &transaction,
                SyncEntityKind::Clip,
                &id,
                SyncChangeKind::Save,
            )?;
            restored.push(load_clip(&transaction, &id)?);
        }
        transaction.commit()?;
        Ok(restored)
    }

    pub fn create_pinboard(&self, name: &str, color: &str) -> Result<Pinboard, StorageError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > TITLE_LIMIT {
            return Err(StorageError::InvalidPinboardName);
        }
        let color = normalize_pinboard_color(color)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let id = PinboardId::new();
        let now = Utc::now();
        let sort_order: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM pinboards",
            [],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO pinboards (
                id, name, color, sort_order, created_at_ms, updated_at_ms, is_shared
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)",
            params![
                id.to_string(),
                name,
                &color,
                sort_order,
                now.timestamp_millis()
            ],
        )?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Pinboard,
            &id.to_string(),
            SyncChangeKind::Save,
        )?;
        transaction.commit()?;
        Ok(Pinboard {
            id,
            name: name.into(),
            color,
            sort_order,
            created_at: now,
            updated_at: now,
            is_shared: false,
            item_count: 0,
        })
    }

    pub fn list_pinboards(&self) -> Result<Vec<Pinboard>, StorageError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT p.id, p.name, p.color, p.sort_order, p.created_at_ms,
                    p.updated_at_ms, p.is_shared, COUNT(c.id)
             FROM pinboards p
             LEFT JOIN pinboard_items pi ON pi.pinboard_id = p.id
             LEFT JOIN clips c ON c.id = pi.clip_id AND c.deleted_at_ms IS NULL
             GROUP BY p.id
             ORDER BY p.sort_order, p.created_at_ms",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RawPinboard {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                sort_order: row.get(3)?,
                created_at_ms: row.get(4)?,
                updated_at_ms: row.get(5)?,
                is_shared: row.get(6)?,
                item_count: row.get(7)?,
            })
        })?;
        rows.map(|row| raw_pinboard(row?)).collect()
    }

    pub fn reserve_owned_pinboard_share(
        &self,
        pinboard_id: PinboardId,
        permission: PinboardSharePermission,
    ) -> Result<PinboardShare, StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let exists = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pinboards WHERE id = ?1)",
            params![pinboard_id.to_string()],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(StorageError::NotFound);
        }
        if let Some(existing) = load_pinboard_share(&transaction, pinboard_id)?
            && existing.state != PinboardShareState::Revoked
        {
            if existing.role != PinboardShareRole::Owner {
                return Err(StorageError::PinboardAlreadyShared);
            }
            return Ok(existing);
        }

        let now = Utc::now();
        let id = uuid::Uuid::new_v4();
        let zone_name = format!("PasteShare_{pinboard_id}");
        transaction.execute(
            "INSERT INTO cloudkit_shares (
                 id, pinboard_id, zone_name, role, permission, state,
                 created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, 'owner', ?4, 'preparing', ?5, ?5)
             ON CONFLICT(pinboard_id) DO UPDATE SET
                 id = excluded.id,
                 zone_name = excluded.zone_name,
                 owner_name = NULL,
                 share_record_name = NULL,
                 share_url = NULL,
                 role = 'owner',
                 permission = excluded.permission,
                 state = 'preparing',
                 server_change_token = NULL,
                 updated_at_ms = excluded.updated_at_ms",
            params![
                id.to_string(),
                pinboard_id.to_string(),
                &zone_name,
                pinboard_share_permission_key(permission),
                now.timestamp_millis()
            ],
        )?;
        transaction.commit()?;
        Ok(PinboardShare {
            id,
            pinboard_id: Some(pinboard_id),
            zone_name,
            owner_name: None,
            share_record_name: None,
            share_url: None,
            role: PinboardShareRole::Owner,
            permission,
            state: PinboardShareState::Preparing,
            server_change_token: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn activate_owned_pinboard_share(
        &self,
        pinboard_id: PinboardId,
        owner_name: &str,
        share_record_name: &str,
        share_url: &str,
    ) -> Result<PinboardShare, StorageError> {
        validate_cloudkit_record_component(owner_name)?;
        validate_cloudkit_record_component(share_record_name)?;
        validate_share_url(share_url)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let now = Utc::now();
        if let Some(existing) = load_pinboard_share(&transaction, pinboard_id)?
            && existing.role == PinboardShareRole::Owner
            && existing.state == PinboardShareState::Active
        {
            if existing.owner_name.as_deref() != Some(owner_name)
                || existing.share_record_name.as_deref() != Some(share_record_name)
                || existing.share_url.as_deref() != Some(share_url)
            {
                return Err(StorageError::PinboardAlreadyShared);
            }
            return Ok(existing);
        }
        let changed = transaction.execute(
            "UPDATE cloudkit_shares
             SET owner_name = ?2, share_record_name = ?3, share_url = ?4,
                 state = 'active', updated_at_ms = ?5
             WHERE pinboard_id = ?1 AND role = 'owner' AND state IN ('preparing', 'active')",
            params![
                pinboard_id.to_string(),
                owner_name,
                share_record_name,
                share_url,
                now.timestamp_millis()
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        transaction.execute(
            "UPDATE pinboards SET is_shared = 1, updated_at_ms = ?2 WHERE id = ?1",
            params![pinboard_id.to_string(), now.timestamp_millis()],
        )?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Pinboard,
            &pinboard_id.to_string(),
            SyncChangeKind::Save,
        )?;
        seed_pinboard_share_outbox(&transaction, pinboard_id)?;
        let share =
            load_pinboard_share(&transaction, pinboard_id)?.ok_or(StorageError::NotFound)?;
        transaction.commit()?;
        Ok(share)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_accepted_pinboard_share(
        &self,
        pinboard_id: PinboardId,
        title: &str,
        color: &str,
        zone_name: &str,
        owner_name: &str,
        share_record_name: &str,
        share_url: &str,
        permission: PinboardSharePermission,
    ) -> Result<PinboardShare, StorageError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > TITLE_LIMIT {
            return Err(StorageError::InvalidPinboardName);
        }
        let color = normalize_pinboard_color(color)?;
        validate_cloudkit_record_component(zone_name)?;
        validate_cloudkit_record_component(owner_name)?;
        validate_cloudkit_record_component(share_record_name)?;
        validate_share_url(share_url)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        match load_pinboard_share(&transaction, pinboard_id)? {
            Some(share)
                if share.role != PinboardShareRole::Participant
                    || share.zone_name != zone_name
                    || share.owner_name.as_deref() != Some(owner_name)
                    || share.share_record_name.as_deref() != Some(share_record_name) =>
            {
                return Err(StorageError::PinboardAlreadyShared);
            }
            None if ensure_exists(&transaction, "pinboards", &pinboard_id.to_string()).is_ok() => {
                return Err(StorageError::PinboardAlreadyShared);
            }
            _ => {}
        }
        if zone_name != format!("PasteShare_{pinboard_id}") {
            return Err(StorageError::InvalidCloudKitShareMetadata);
        }
        let now = Utc::now();
        let sort_order: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM pinboards",
            [],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO pinboards (
                 id, name, color, sort_order, created_at_ms, updated_at_ms, is_shared
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1)
             ON CONFLICT(id) DO UPDATE SET is_shared = 1, updated_at_ms = excluded.updated_at_ms",
            params![
                pinboard_id.to_string(),
                title,
                &color,
                sort_order,
                now.timestamp_millis()
            ],
        )?;
        let id = uuid::Uuid::new_v4();
        transaction.execute(
            "INSERT INTO cloudkit_shares (
                 id, pinboard_id, zone_name, owner_name, share_record_name, share_url,
                 role, permission, state, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'participant', ?7, 'active', ?8, ?8)
             ON CONFLICT(pinboard_id) DO UPDATE SET
                 zone_name = excluded.zone_name,
                 owner_name = excluded.owner_name,
                 share_record_name = excluded.share_record_name,
                 share_url = excluded.share_url,
                 role = 'participant',
                 permission = excluded.permission,
                 state = 'active',
                 updated_at_ms = excluded.updated_at_ms",
            params![
                id.to_string(),
                pinboard_id.to_string(),
                zone_name,
                owner_name,
                share_record_name,
                share_url,
                pinboard_share_permission_key(permission),
                now.timestamp_millis()
            ],
        )?;
        let share =
            load_pinboard_share(&transaction, pinboard_id)?.ok_or(StorageError::NotFound)?;
        transaction.commit()?;
        Ok(share)
    }

    pub fn list_pinboard_shares(&self) -> Result<Vec<PinboardShare>, StorageError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, pinboard_id, zone_name, owner_name, share_record_name, share_url,
                    role, permission, state, server_change_token, created_at_ms, updated_at_ms
             FROM cloudkit_shares
             WHERE state != 'revoked'
             ORDER BY updated_at_ms DESC, id",
        )?;
        let rows = statement.query_map([], raw_pinboard_share_row)?;
        rows.map(|row| parse_pinboard_share(row?)).collect()
    }

    pub fn update_pinboard(
        &self,
        id: PinboardId,
        name: &str,
        color: &str,
    ) -> Result<Pinboard, StorageError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > TITLE_LIMIT {
            return Err(StorageError::InvalidPinboardName);
        }
        let color = normalize_pinboard_color(color)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_pinboard_share_writable(&transaction, id)?;
        let changed = transaction.execute(
            "UPDATE pinboards
             SET name = ?2, color = ?3, updated_at_ms = ?4
             WHERE id = ?1",
            params![id.to_string(), name, &color, Utc::now().timestamp_millis()],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Pinboard,
            &id.to_string(),
            SyncChangeKind::Save,
        )?;
        transaction.commit()?;
        drop(connection);
        self.list_pinboards()?
            .into_iter()
            .find(|pinboard| pinboard.id == id)
            .ok_or(StorageError::NotFound)
    }

    pub fn reorder_pinboards(
        &self,
        ordered_ids: &[PinboardId],
    ) -> Result<Vec<Pinboard>, StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let existing = {
            let mut statement = transaction.prepare("SELECT id FROM pinboards")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<BTreeSet<_>, _>>()?
        };
        let requested = ordered_ids
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        if existing != requested || requested.len() != ordered_ids.len() {
            return Err(StorageError::InvalidPinboardOrder);
        }

        let now = Utc::now().timestamp_millis();
        for (sort_order, id) in ordered_ids.iter().enumerate() {
            ensure_pinboard_share_writable(&transaction, *id)?;
            transaction.execute(
                "UPDATE pinboards SET sort_order = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![id.to_string(), to_i64(sort_order)?, now],
            )?;
            enqueue_sync_change(
                &transaction,
                SyncEntityKind::Pinboard,
                &id.to_string(),
                SyncChangeKind::Save,
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.list_pinboards()
    }

    pub fn pin_clip(&self, pinboard_id: PinboardId, clip_id: ClipId) -> Result<(), StorageError> {
        self.pin_clips(pinboard_id, &[clip_id])
    }

    pub fn pin_clips(
        &self,
        pinboard_id: PinboardId,
        clip_ids: &[ClipId],
    ) -> Result<(), StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        pin_clips_transaction(&transaction, pinboard_id, clip_ids)?;
        transaction.commit()?;
        Ok(())
    }

    /// Moves a selection into one Pinboard at a stable anchor. The store loads
    /// the complete board order, so filtered/paged UI results cannot discard
    /// invisible members. All source removals, inserts and order changes commit
    /// together with their sync operations.
    pub fn place_pinboard_clips(
        &self,
        pinboard_id: PinboardId,
        clip_ids: &[ClipId],
        anchor: Option<ClipId>,
        after: bool,
    ) -> Result<bool, StorageError> {
        let unique = clip_ids.iter().copied().collect::<BTreeSet<_>>();
        if clip_ids.is_empty() || clip_ids.len() > 200 || unique.len() != clip_ids.len() {
            return Err(StorageError::InvalidPinboardPlacement);
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_exists(&transaction, "pinboards", &pinboard_id.to_string())?;
        ensure_pinboard_share_writable(&transaction, pinboard_id)?;
        let previous = load_pinboard_clip_order(&transaction, pinboard_id)?;
        for clip_id in clip_ids {
            ensure_exists(&transaction, "clips", &clip_id.to_string())?;
            ensure_clip_share_writable(&transaction, &clip_id.to_string())?;
        }
        if let Some(anchor) = anchor {
            if !previous.contains(&anchor) {
                return Err(StorageError::NotFound);
            }
            if unique.contains(&anchor) {
                return Ok(false);
            }
        }
        let membership_changed = pin_clips_transaction(&transaction, pinboard_id, clip_ids)?;
        let mut ordered = previous
            .iter()
            .copied()
            .filter(|id| !unique.contains(id))
            .collect::<Vec<_>>();
        let insertion = anchor.map_or(ordered.len(), |anchor| {
            ordered
                .iter()
                .position(|id| *id == anchor)
                .expect("anchor was validated")
                + usize::from(after)
        });
        ordered.splice(insertion..insertion, clip_ids.iter().copied());
        if !membership_changed && ordered == previous {
            return Ok(false);
        }
        for (position, id) in ordered.iter().enumerate() {
            let changed = transaction.execute(
                "UPDATE pinboard_items SET position = ?3
                 WHERE pinboard_id = ?1 AND clip_id = ?2 AND position != ?3",
                params![pinboard_id.to_string(), id.to_string(), to_i64(position)?],
            )?;
            if changed > 0 {
                enqueue_sync_change(
                    &transaction,
                    SyncEntityKind::PinboardMembership,
                    &pinboard_membership_id(pinboard_id, *id),
                    SyncChangeKind::Save,
                )?;
            }
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn unpin_clip(&self, pinboard_id: PinboardId, clip_id: ClipId) -> Result<(), StorageError> {
        self.unpin_clips(pinboard_id, &[clip_id])
    }

    pub fn unpin_clips(
        &self,
        pinboard_id: PinboardId,
        clip_ids: &[ClipId],
    ) -> Result<(), StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_exists(&transaction, "pinboards", &pinboard_id.to_string())?;
        ensure_pinboard_share_writable(&transaction, pinboard_id)?;
        for clip_id in clip_ids {
            let changed = transaction.execute(
                "DELETE FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2",
                params![pinboard_id.to_string(), clip_id.to_string()],
            )?;
            if changed > 0 {
                let membership_id = pinboard_membership_id(pinboard_id, *clip_id);
                enqueue_sync_change(
                    &transaction,
                    SyncEntityKind::PinboardMembership,
                    &membership_id,
                    SyncChangeKind::Delete,
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn move_pinboard_item(
        &self,
        pinboard_id: PinboardId,
        clip_id: ClipId,
        direction: i8,
    ) -> Result<bool, StorageError> {
        if !matches!(direction, -1 | 1) {
            return Err(StorageError::InvalidPinboardItemMove);
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_pinboard_share_writable(&transaction, pinboard_id)?;
        let position = transaction
            .query_row(
                "SELECT position FROM pinboard_items
                 WHERE pinboard_id = ?1 AND clip_id = ?2",
                params![pinboard_id.to_string(), clip_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(StorageError::NotFound)?;
        let comparison = if direction < 0 { "<" } else { ">" };
        let ordering = if direction < 0 { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT clip_id, position FROM pinboard_items
             WHERE pinboard_id = ?1 AND position {comparison} ?2
             ORDER BY position {ordering}, created_at_ms {ordering} LIMIT 1"
        );
        let neighbor = transaction
            .query_row(&sql, params![pinboard_id.to_string(), position], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .optional()?;
        let Some((neighbor_id, neighbor_position)) = neighbor else {
            return Ok(false);
        };
        transaction.execute(
            "UPDATE pinboard_items SET position = ?3
             WHERE pinboard_id = ?1 AND clip_id = ?2",
            params![
                pinboard_id.to_string(),
                clip_id.to_string(),
                neighbor_position
            ],
        )?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::PinboardMembership,
            &pinboard_membership_id(pinboard_id, clip_id),
            SyncChangeKind::Save,
        )?;
        let neighbor_clip_id = ClipId::from_str(&neighbor_id)
            .map_err(|error| corrupt("pinboard neighbor clip id", error))?;
        transaction.execute(
            "UPDATE pinboard_items SET position = ?3
             WHERE pinboard_id = ?1 AND clip_id = ?2",
            params![pinboard_id.to_string(), neighbor_id, position],
        )?;
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::PinboardMembership,
            &pinboard_membership_id(pinboard_id, neighbor_clip_id),
            SyncChangeKind::Save,
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn delete_pinboard(&self, id: PinboardId) -> Result<(), StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        ensure_pinboard_share_writable(&transaction, id)?;
        // Deleting the board would cascade its pending share deliveries. A
        // dedicated revoke/leave operation must finish before local removal.
        if load_pinboard_share(&transaction, id)?
            .is_some_and(|share| share.state != PinboardShareState::Revoked)
        {
            return Err(StorageError::PinboardAlreadyShared);
        }
        let clip_ids = {
            let mut statement =
                transaction.prepare("SELECT clip_id FROM pinboard_items WHERE pinboard_id = ?1")?;
            statement
                .query_map(params![id.to_string()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for clip_id in clip_ids {
            let clip_id = ClipId::from_str(&clip_id)
                .map_err(|error| corrupt("pinboard item clip id", error))?;
            enqueue_sync_change(
                &transaction,
                SyncEntityKind::PinboardMembership,
                &pinboard_membership_id(id, clip_id),
                SyncChangeKind::Delete,
            )?;
        }
        let changed = transaction.execute(
            "DELETE FROM pinboards WHERE id = ?1",
            params![id.to_string()],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        enqueue_sync_change(
            &transaction,
            SyncEntityKind::Pinboard,
            &id.to_string(),
            SyncChangeKind::Delete,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn apply_retention(
        &self,
        policy: RetentionPolicy,
        now: DateTime<Utc>,
    ) -> Result<usize, StorageError> {
        policy.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let removed = apply_retention_transaction(&transaction, policy, now)?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn blob_count(&self) -> Result<u64, StorageError> {
        let connection = self.lock()?;
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
        u64::try_from(count).map_err(|error| corrupt("blob count", error))
    }

    pub fn get_or_create_device(
        &self,
        display_name: impl Into<String>,
    ) -> Result<DeviceMetadata, StorageError> {
        let display_name = display_name.into();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let stored_id = transaction
            .query_row(
                "SELECT value FROM settings WHERE key = 'device_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let id = match stored_id {
            Some(value) => {
                DeviceId::from_str(&value).map_err(|error| corrupt("stored device id", error))?
            }
            None => {
                let id = DeviceId::new();
                transaction.execute(
                    "INSERT INTO settings (key, value, updated_at_ms) VALUES ('device_id', ?1, ?2)",
                    params![id.to_string(), Utc::now().timestamp_millis()],
                )?;
                id
            }
        };
        transaction.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES ('device_display_name', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
            params![display_name, Utc::now().timestamp_millis()],
        )?;
        transaction.commit()?;
        Ok(DeviceMetadata { id, display_name })
    }

    pub fn load_capture_preferences(&self) -> Result<CapturePreferences, StorageError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'capture_preferences'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value.map_or_else(
            || Ok(CapturePreferences::default()),
            |json| {
                serde_json::from_str::<CapturePreferences>(&json)
                    .map_err(|error| corrupt("capture preferences", error))?
                    .normalized()
                    .map_err(Into::into)
            },
        )
    }

    pub fn save_capture_preferences(
        &self,
        preferences: CapturePreferences,
    ) -> Result<CapturePreferences, StorageError> {
        let preferences = preferences.normalized()?;
        let json = serde_json::to_string(&preferences)
            .map_err(|error| corrupt("capture preferences", error))?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES ('capture_preferences', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
            params![json, Utc::now().timestamp_millis()],
        )?;
        Ok(preferences)
    }

    pub fn load_desktop_preferences(&self) -> Result<DesktopPreferences, StorageError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'desktop_preferences'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value.map_or_else(
            || Ok(DesktopPreferences::default()),
            |json| {
                serde_json::from_str::<DesktopPreferences>(&json)
                    .map_err(|error| corrupt("desktop preferences", error))
            },
        )
    }

    /// Update only language while holding the settings lock. Unrelated saved
    /// preferences cannot be overwritten by a UI draft or a concurrent update.
    pub fn save_language(
        &self,
        language: paste_domain::Language,
    ) -> Result<paste_domain::Language, StorageError> {
        let connection = self.lock()?;
        let json = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'desktop_preferences'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut preferences = match json {
            Some(json) => serde_json::from_str::<DesktopPreferences>(&json)
                .map_err(|error| corrupt("desktop preferences", error))?,
            None => DesktopPreferences::default(),
        };
        preferences.language = language;
        let json = serde_json::to_string(&preferences)
            .map_err(|error| corrupt("desktop preferences", error))?;
        connection.execute("INSERT INTO settings (key, value, updated_at_ms) VALUES ('desktop_preferences', ?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms", params![json, Utc::now().timestamp_millis()])?;
        Ok(language)
    }

    pub fn save_desktop_preferences(
        &self,
        preferences: DesktopPreferences,
    ) -> Result<DesktopPreferences, StorageError> {
        let json = serde_json::to_string(&preferences)
            .map_err(|error| corrupt("desktop preferences", error))?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES ('desktop_preferences', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
            params![json, Utc::now().timestamp_millis()],
        )?;
        Ok(preferences)
    }

    pub fn load_cached_preview(
        &self,
        clip_id: ClipId,
        source_content_hash: &[u8; 32],
    ) -> Result<Option<CachedPreview>, StorageError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT media_type, bytes, pixel_width, pixel_height
                 FROM preview_cache
                 WHERE clip_id = ?1 AND source_content_hash = ?2",
                params![clip_id.to_string(), source_content_hash.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(media_type, bytes, pixel_width, pixel_height)| {
                Ok(CachedPreview {
                    media_type,
                    bytes,
                    pixel_width: u32::try_from(pixel_width)
                        .map_err(|error| corrupt("preview pixel width", error))?,
                    pixel_height: u32::try_from(pixel_height)
                        .map_err(|error| corrupt("preview pixel height", error))?,
                })
            })
            .transpose()
    }

    pub fn save_cached_preview(
        &self,
        clip_id: ClipId,
        source_content_hash: &[u8; 32],
        preview: &CachedPreview,
    ) -> Result<(), StorageError> {
        let connection = self.lock()?;
        ensure_exists(&connection, "clips", &clip_id.to_string())?;
        connection.execute(
            "INSERT INTO preview_cache (
                 clip_id, source_content_hash, media_type, bytes,
                 pixel_width, pixel_height, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(clip_id) DO UPDATE SET
                 source_content_hash = excluded.source_content_hash,
                 media_type = excluded.media_type,
                 bytes = excluded.bytes,
                 pixel_width = excluded.pixel_width,
                 pixel_height = excluded.pixel_height,
                 created_at_ms = excluded.created_at_ms",
            params![
                clip_id.to_string(),
                source_content_hash.as_slice(),
                &preview.media_type,
                &preview.bytes,
                i64::from(preview.pixel_width),
                i64::from(preview.pixel_height),
                Utc::now().timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn pending_sync_changes(&self, limit: u32) -> Result<Vec<SyncChange>, StorageError> {
        if !(1..=250).contains(&limit) {
            return Err(StorageError::InvalidSyncBatchLimit);
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT change_json FROM sync_outbox
             WHERE state = 'pending' AND NOT EXISTS (SELECT 1 FROM shared_only_outbox x WHERE x.operation_id = sync_outbox.operation_id)
             ORDER BY created_at_ms, logical_counter, node_id, operation_id
             LIMIT ?1",
        )?;
        statement
            .query_map(params![limit], |row| row.get::<_, String>(0))?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| corrupt("sync change", error))
            })
            .collect()
    }

    pub fn prepare_private_sync_batch(
        &self,
        limit: u32,
    ) -> Result<PreparedSyncBatch, StorageError> {
        if !(1..=250).contains(&limit) {
            return Err(StorageError::InvalidSyncBatchLimit);
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT change_json FROM sync_outbox
             WHERE state = 'pending' AND NOT EXISTS (SELECT 1 FROM shared_only_outbox x WHERE x.operation_id = sync_outbox.operation_id)
             ORDER BY created_at_ms, logical_counter, node_id, operation_id
             LIMIT ?1",
        )?;
        let changes = statement
            .query_map(params![limit], |row| row.get::<_, String>(0))?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| corrupt("sync change", error))
            })
            .collect::<Result<Vec<SyncChange>, _>>()?;
        drop(statement);

        let mut blob_bytes = BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut operations = Vec::new();
        for group in coalesce_changes(&changes) {
            let (schema_version, payload) =
                load_sync_snapshot(&connection, &group.change, &mut blob_bytes)?;
            let envelope = SyncEnvelope {
                schema_version,
                scope: SyncScope::Private,
                change: group.change,
                payload,
            };
            envelope
                .validate()
                .map_err(|error| corrupt("sync envelope", error))?;
            operations.push(PreparedSyncOperation {
                envelope,
                acknowledge_operation_ids: group.operation_ids,
            });
        }
        Ok(PreparedSyncBatch {
            operations,
            blobs: blob_bytes
                .into_iter()
                .map(|(content_hash, bytes)| SyncBlob {
                    content_hash,
                    bytes,
                })
                .collect(),
        })
    }

    pub fn pending_pinboard_share_change_count(
        &self,
        share_id: uuid::Uuid,
    ) -> Result<u64, StorageError> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM cloudkit_share_outbox
             WHERE share_id = ?1 AND state = 'pending'",
            params![share_id.to_string()],
            |row| row.get(0),
        )?;
        u64::try_from(count).map_err(|error| corrupt("pending shared change count", error))
    }

    pub fn prepare_pinboard_share_sync_batch(
        &self,
        share_id: uuid::Uuid,
        limit: u32,
    ) -> Result<PreparedSyncBatch, StorageError> {
        if !(1..=250).contains(&limit) {
            return Err(StorageError::InvalidSyncBatchLimit);
        }
        let connection = self.lock()?;
        let writable: Option<bool> = connection
            .query_row(
                "SELECT role = 'owner' OR permission = 'read_write'
                 FROM cloudkit_shares WHERE id = ?1 AND state = 'active'",
                params![share_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match writable {
            Some(true) => {}
            Some(false) => return Err(StorageError::ReadOnlyPinboardShare),
            None => return Err(StorageError::NotFound),
        }
        let mut statement = connection.prepare(
            "SELECT w.envelope_json
             FROM cloudkit_share_outbox so
             JOIN sync_outbox o ON o.operation_id = so.operation_id
             JOIN shared_wire_outbox w ON w.share_id = so.share_id AND w.operation_id = so.operation_id
             WHERE so.share_id = ?1 AND so.state = 'pending'
             ORDER BY so.created_at_ms, o.logical_counter, o.node_id, o.operation_id
             LIMIT ?2",
        )?;
        let envelopes = statement
            .query_map(params![share_id.to_string(), limit], |row| {
                row.get::<_, String>(0)
            })?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| corrupt("shared sync change", error))
            })
            .collect::<Result<Vec<SyncEnvelope>, _>>()?;
        drop(statement);

        let changes = envelopes
            .iter()
            .map(|e| e.change.clone())
            .collect::<Vec<_>>();
        let mut blob_bytes = BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut operations = Vec::new();
        for group in coalesce_changes(&changes) {
            let envelope = envelopes
                .iter()
                .find(|e| e.change.operation_id == group.change.operation_id)
                .ok_or(StorageError::UnknownSharedSyncOperation)?
                .clone();
            // Snapshot lookup is keyed by operation UUID. Its local entity ID
            // need not equal the frozen wire ID and is never retranslated here.
            load_sync_snapshot(&connection, &group.change, &mut blob_bytes)?;
            envelope
                .validate()
                .map_err(|error| corrupt("shared sync envelope", error))?;
            operations.push(PreparedSyncOperation {
                envelope,
                acknowledge_operation_ids: group.operation_ids,
            });
        }
        Ok(PreparedSyncBatch {
            operations,
            blobs: blob_bytes
                .into_iter()
                .map(|(content_hash, bytes)| SyncBlob {
                    content_hash,
                    bytes,
                })
                .collect(),
        })
    }

    pub fn acknowledge_pinboard_share_changes(
        &self,
        share_id: uuid::Uuid,
        operation_ids: &[uuid::Uuid],
    ) -> Result<(), StorageError> {
        if operation_ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let now_ms = Utc::now().timestamp_millis();
        for operation_id in operation_ids {
            let changed = transaction.execute(
                "UPDATE cloudkit_share_outbox
                 SET state = 'acknowledged', acknowledged_at_ms = COALESCE(acknowledged_at_ms, ?1)
                 WHERE share_id = ?2 AND operation_id = ?3",
                params![now_ms, share_id.to_string(), operation_id.to_string()],
            )?;
            if changed == 0 {
                return Err(StorageError::UnknownSharedSyncOperation);
            }
        }
        delete_unreferenced_blobs(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn apply_remote_sync_batch(
        &self,
        envelopes: &[SyncEnvelope],
        blobs: &[SyncBlob],
    ) -> Result<RemoteApplyReport, StorageError> {
        self.apply_private_sync_batch(envelopes, blobs, None)
    }

    /// Applies a downloaded page, defers missing relations and checkpoints
    /// its token in one transaction. No token can skip an uncommitted page.
    pub fn apply_private_sync_page(
        &self,
        envelopes: &[SyncEnvelope],
        blobs: &[SyncBlob],
        server_change_token: &[u8],
    ) -> Result<RemoteApplyReport, StorageError> {
        if server_change_token.is_empty() || server_change_token.len() > 4 * 1024 * 1024 {
            return Err(StorageError::InvalidSyncToken);
        }
        self.apply_private_sync_batch(envelopes, blobs, Some(server_change_token))
    }

    pub fn pending_remote_membership_count(&self) -> Result<usize, StorageError> {
        let connection = self.lock()?;
        deferred_membership_count(&connection)
    }

    fn apply_private_sync_batch(
        &self,
        envelopes: &[SyncEnvelope],
        blobs: &[SyncBlob],
        server_change_token: Option<&[u8]>,
    ) -> Result<RemoteApplyReport, StorageError> {
        if envelopes.len() > 250 || blobs.len() > REMOTE_SYNC_BATCH_BLOB_LIMIT {
            return Err(StorageError::InvalidRemoteSyncBatch);
        }
        let mut incoming_blobs = BTreeMap::<[u8; 32], &[u8]>::new();
        let mut total_blob_bytes = 0_usize;
        for blob in blobs {
            if blob.bytes.len() > REMOTE_SYNC_BLOB_LIMIT_BYTES {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            total_blob_bytes = total_blob_bytes
                .checked_add(blob.bytes.len())
                .ok_or(StorageError::RemoteSyncPayloadTooLarge)?;
            if total_blob_bytes > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            if *blake3::hash(&blob.bytes).as_bytes() != blob.content_hash {
                return Err(StorageError::CorruptData(
                    "remote sync blob hash does not match its bytes".into(),
                ));
            }
            if let Some(existing) = incoming_blobs.insert(blob.content_hash, &blob.bytes)
                && existing != blob.bytes
            {
                return Err(StorageError::CorruptData(
                    "remote sync batch contains conflicting bytes for one blob hash".into(),
                ));
            }
        }
        for envelope in envelopes {
            // Shared data must eventually enter through a share-scoped mapper;
            // the private history API must never trust participant-chosen IDs.
            if envelope.scope != SyncScope::Private {
                return Err(StorageError::InvalidRemoteSyncBatch);
            }
            envelope
                .validate()
                .map_err(|error| corrupt("remote sync envelope", error))?;
        }

        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let mut report = RemoteApplyReport::default();
        let mut ordered = envelopes.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|envelope| remote_apply_priority(envelope));
        for envelope in ordered {
            if share_materialize::private_forbidden(&transaction, &envelope.change.entity)? {
                return Err(StorageError::InvalidRemoteSyncBatch);
            }
            let local = load_sync_entity_change(&transaction, &envelope.change.entity)?;
            let decision = decide_remote_change(local.as_ref(), &envelope.change);
            if decision.preserve_losing_clip
                && let Some(local) = local
            {
                let conflict = RemoteConflict {
                    local,
                    remote: envelope.clone(),
                    decision,
                };
                store_sync_conflict(&transaction, &conflict, &incoming_blobs)?;
                report.conflicts.push(conflict);
                continue;
            }
            if decision.disposition == RemoteDisposition::Ignore {
                report.ignored += 1;
                continue;
            }
            if let Some(SyncPayload::PinboardMembership(membership)) = &envelope.payload
                && !membership_dependencies_ready(&transaction, membership)?
            {
                defer_remote_membership(&transaction, envelope, membership)?;
            } else {
                apply_remote_envelope(&transaction, envelope, &incoming_blobs)?;
                if envelope.change.entity.kind == SyncEntityKind::PinboardMembership {
                    clear_deferred_membership(&transaction, &envelope.change.entity.id)?;
                }
                report.applied += 1;
            }
            store_remote_sync_change(&transaction, &envelope.change)?;
            observe_remote_sync_clock(&transaction, &envelope.change.timestamp)?;
        }
        report.applied += drain_deferred_memberships(&transaction)?;
        report.deferred = deferred_membership_count(&transaction)?;
        if let Some(token) = server_change_token {
            transaction.execute(
                "INSERT INTO sync_tokens (scope, token, updated_at_ms) VALUES ('private', ?1, ?2)
                 ON CONFLICT(scope) DO UPDATE SET token = excluded.token, updated_at_ms = excluded.updated_at_ms",
                params![token, Utc::now().timestamp_millis()],
            )?;
        }
        delete_unreferenced_blobs(&transaction)?;
        transaction.commit()?;
        Ok(report)
    }

    pub fn acknowledge_sync_changes(
        &self,
        operation_ids: &[uuid::Uuid],
    ) -> Result<(), StorageError> {
        if operation_ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let now_ms = Utc::now().timestamp_millis();
        for operation_id in operation_ids {
            let changed = transaction.execute(
                "UPDATE sync_outbox
                 SET state = 'acknowledged', acknowledged_at_ms = ?1
                 WHERE operation_id = ?2 AND NOT EXISTS (SELECT 1 FROM shared_only_outbox x WHERE x.operation_id = sync_outbox.operation_id)",
                params![now_ms, operation_id.to_string()],
            )?;
            if changed == 0 {
                return Err(StorageError::UnknownSyncOperation);
            }
        }
        delete_unreferenced_blobs(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_sync_token(&self, scope: &str, token: &[u8]) -> Result<(), StorageError> {
        validate_sync_scope(scope)?;
        if token.is_empty() || token.len() > 4 * 1024 * 1024 {
            return Err(StorageError::InvalidSyncToken);
        }
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO sync_tokens (scope, token, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(scope) DO UPDATE SET
                 token = excluded.token,
                 updated_at_ms = excluded.updated_at_ms",
            params![scope, token, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn load_sync_token(&self, scope: &str) -> Result<Option<Vec<u8>>, StorageError> {
        validate_sync_scope(scope)?;
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT token FROM sync_tokens WHERE scope = ?1",
                params![scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn clear_sync_token(&self, scope: &str) -> Result<(), StorageError> {
        validate_sync_scope(scope)?;
        let connection = self.lock()?;
        connection.execute("DELETE FROM sync_tokens WHERE scope = ?1", params![scope])?;
        Ok(())
    }

    pub fn pending_sync_change_count(&self) -> Result<u64, StorageError> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE state = 'pending'
             AND NOT EXISTS (SELECT 1 FROM shared_only_outbox x WHERE x.operation_id = sync_outbox.operation_id)",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(count).map_err(|error| corrupt("pending sync change count", error))
    }

    pub fn pending_sync_conflict_count(&self) -> Result<u64, StorageError> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE resolved_at_ms IS NULL",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(count).map_err(|error| corrupt("pending sync conflict count", error))
    }

    pub fn list_sync_conflicts(
        &self,
        limit: u32,
    ) -> Result<Vec<SyncConflictSummary>, StorageError> {
        if limit == 0 || limit > 100 {
            return Err(StorageError::InvalidSyncConflictLimit);
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sc.id, sc.entity_id, c.title, sc.remote_envelope_json,
                    sc.remote_would_win, sc.created_at_ms
             FROM sync_conflicts sc
             LEFT JOIN clips c ON c.id = sc.entity_id
             WHERE sc.resolved_at_ms IS NULL
             ORDER BY sc.created_at_ms DESC, sc.id DESC
             LIMIT ?1",
        )?;
        let raw = statement
            .query_map(params![i64::from(limit)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raw.into_iter()
            .map(
                |(id, entity_id, local_title, remote_json, remote_would_win, created_at_ms)| {
                    let remote: SyncEnvelope = serde_json::from_str(&remote_json)
                        .map_err(|error| corrupt("stored remote sync conflict", error))?;
                    let remote_title = match remote.payload {
                        Some(SyncPayload::Clip(item)) => item.title,
                        None => "已在另一台设备删除".into(),
                        _ => {
                            return Err(StorageError::CorruptData(
                                "non-clip entity was stored as a sync conflict".into(),
                            ));
                        }
                    };
                    Ok(SyncConflictSummary {
                        id: uuid::Uuid::parse_str(&id)
                            .map_err(|error| corrupt("sync conflict id", error))?,
                        clip_id: ClipId::from_str(&entity_id)
                            .map_err(|error| corrupt("sync conflict clip id", error))?,
                        local_title: local_title.unwrap_or_else(|| "已在本机删除".into()),
                        remote_title,
                        remote_would_win,
                        created_at: timestamp(created_at_ms)?,
                    })
                },
            )
            .collect()
    }

    pub fn resolve_sync_conflict(
        &self,
        conflict_id: uuid::Uuid,
        resolution: SyncConflictResolution,
    ) -> Result<(), StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let raw = transaction
            .query_row(
                "SELECT local_change_json, remote_envelope_json, resolved_at_ms
                 FROM sync_conflicts
                 WHERE id = ?1",
                params![conflict_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StorageError::NotFound)?;
        // A committed first choice wins. Retrying after a lost response must
        // not emit another operation or require already-released attachments.
        if raw.2.is_some() {
            return Ok(());
        }
        let local: SyncChange = serde_json::from_str(&raw.0)
            .map_err(|error| corrupt("stored local sync conflict", error))?;
        let remote: SyncEnvelope = serde_json::from_str(&raw.1)
            .map_err(|error| corrupt("stored remote sync conflict", error))?;
        remote
            .validate()
            .map_err(|error| corrupt("stored remote sync conflict", error))?;
        if local.entity != remote.change.entity
            || local.entity.kind != SyncEntityKind::Clip
            || remote.scope != SyncScope::Private
        {
            return Err(StorageError::CorruptData(
                "sync conflict entity metadata does not match".into(),
            ));
        }

        ensure_clip_share_writable(&transaction, &local.entity.id)?;
        let current = load_sync_entity_change(&transaction, &local.entity)?
            .ok_or_else(|| corrupt("sync conflict", "current entity version is missing"))?;
        let mut merged = current.version.clone();
        merged.merge(&local.version);
        merged.merge(&remote.change.version);
        let chosen_kind = match resolution {
            // The local title shown in the UI is live. Keep the current state,
            // including edits, delete/undo and previous conflict choices made
            // after this conflict was first recorded.
            SyncConflictResolution::KeepLocal => current.change,
            SyncConflictResolution::AcceptRemote => {
                let blobs = load_sync_conflict_blobs(&transaction, conflict_id)?;
                let blob_refs = blobs
                    .iter()
                    .map(|(hash, bytes)| (*hash, bytes.as_slice()))
                    .collect();
                apply_remote_envelope(&transaction, &remote, &blob_refs)?;
                remote.change.change
            }
        };
        let version_json = serde_json::to_string(&merged)
            .map_err(|error| corrupt("merged conflict version", error))?;
        transaction.execute(
            "UPDATE sync_entity_versions SET version_json = ?1
             WHERE entity_kind = ?2 AND entity_id = ?3",
            params![
                version_json,
                sync_entity_kind_key(local.entity.kind),
                &local.entity.id
            ],
        )?;
        observe_remote_sync_clock(&transaction, &remote.change.timestamp)?;
        // Both choices are new causal events. Reusing the remote event would
        // not override a losing local version already seen by a third device.
        let choice = enqueue_sync_change(
            &transaction,
            local.entity.kind,
            &local.entity.id,
            chosen_kind,
        )?;
        supersede_clip_outbox(&transaction, &local.entity.id, choice.operation_id)?;
        transaction.execute(
            "UPDATE sync_conflicts SET resolved_at_ms = ?1 WHERE id = ?2",
            params![Utc::now().timestamp_millis(), conflict_id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM sync_conflict_blobs WHERE conflict_id = ?1",
            params![conflict_id.to_string()],
        )?;
        delete_unreferenced_blobs(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn cloud_sync_enabled(&self) -> Result<bool, StorageError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'cloud_sync_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value.as_deref() == Some("true"))
    }

    pub fn set_cloud_sync_enabled(&self, enabled: bool) -> Result<(), StorageError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES ('cloud_sync_enabled', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at_ms = excluded.updated_at_ms",
            params![
                if enabled { "true" } else { "false" },
                Utc::now().timestamp_millis()
            ],
        )?;
        Ok(())
    }

    pub fn mcp_enabled(&self) -> Result<bool, StorageError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'mcp_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value.as_deref() == Some("true"))
    }

    pub fn set_mcp_enabled(&self, enabled: bool) -> Result<(), StorageError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES ('mcp_enabled', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at_ms = excluded.updated_at_ms",
            params![
                if enabled { "true" } else { "false" },
                Utc::now().timestamp_millis()
            ],
        )?;
        Ok(())
    }

    pub fn register_mcp_client(
        &self,
        display_name: &str,
        token_hash: &[u8; 32],
    ) -> Result<McpClient, StorageError> {
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > TITLE_LIMIT {
            return Err(StorageError::InvalidMcpClientName);
        }
        let id = uuid::Uuid::new_v4();
        let now = Utc::now();
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO mcp_clients (
                 id, display_name, token_hash, created_at_ms, last_used_at_ms, revoked_at_ms
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
            params![
                id.to_string(),
                display_name,
                token_hash.as_slice(),
                now.timestamp_millis()
            ],
        )?;
        Ok(McpClient {
            id,
            display_name: display_name.to_owned(),
            created_at: now,
            last_used_at: None,
        })
    }

    pub fn list_mcp_clients(&self) -> Result<Vec<McpClient>, StorageError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, display_name, created_at_ms, last_used_at_ms
             FROM mcp_clients
             WHERE revoked_at_ms IS NULL
             ORDER BY created_at_ms DESC, id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?
            .map(|row| {
                let (id, display_name, created_at_ms, last_used_at_ms) = row?;
                Ok(McpClient {
                    id: uuid::Uuid::parse_str(&id)
                        .map_err(|error| corrupt("MCP client id", error))?,
                    display_name,
                    created_at: timestamp(created_at_ms)?,
                    last_used_at: last_used_at_ms.map(timestamp).transpose()?,
                })
            })
            .collect()
    }

    pub fn authorize_mcp_token(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<McpClient>, StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let enabled = transaction
            .query_row(
                "SELECT value FROM settings WHERE key = 'mcp_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some("true");
        if !enabled {
            return Ok(None);
        }
        let raw = transaction
            .query_row(
                "SELECT id, display_name, created_at_ms, last_used_at_ms
                 FROM mcp_clients
                 WHERE token_hash = ?1 AND revoked_at_ms IS NULL",
                params![token_hash.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, display_name, created_at_ms, _last_used_at_ms)) = raw else {
            return Ok(None);
        };
        let now = Utc::now();
        transaction.execute(
            "UPDATE mcp_clients SET last_used_at_ms = ?1 WHERE id = ?2",
            params![now.timestamp_millis(), &id],
        )?;
        transaction.commit()?;
        Ok(Some(McpClient {
            id: uuid::Uuid::parse_str(&id).map_err(|error| corrupt("MCP client id", error))?,
            display_name,
            created_at: timestamp(created_at_ms)?,
            last_used_at: Some(now),
        }))
    }

    pub fn revoke_mcp_client(&self, id: uuid::Uuid) -> Result<(), StorageError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE mcp_clients SET revoked_at_ms = ?1
             WHERE id = ?2 AND revoked_at_ms IS NULL",
            params![Utc::now().timestamp_millis(), id.to_string()],
        )?;
        if changed == 0 {
            Err(StorageError::NotFound)
        } else {
            Ok(())
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }
}

fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    loop {
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => connection.execute_batch(MIGRATION_V1)?,
            1 => connection.execute_batch(MIGRATION_V2)?,
            2 => connection.execute_batch(MIGRATION_V3)?,
            3 => connection.execute_batch(MIGRATION_V4)?,
            4 => connection.execute_batch(MIGRATION_V5)?,
            5 => connection.execute_batch(MIGRATION_V6)?,
            6 => connection.execute_batch(MIGRATION_V7)?,
            7 => connection.execute_batch(MIGRATION_V8)?,
            8 => connection.execute_batch(MIGRATION_V9)?,
            9 => connection.execute_batch(MIGRATION_V10)?,
            10 => connection.execute_batch(MIGRATION_V11)?,
            11 => connection.execute_batch(MIGRATION_V12)?,
            12 => {
                let transaction = connection.transaction()?;
                transaction.execute_batch(MIGRATION_V13)?;
                transaction.execute_batch(MIGRATION_V14)?;
                // The renewal code reads representation metadata using the
                // current schema, but never guesses a lost legacy native type.
                transaction.execute_batch("ALTER TABLE representations ADD COLUMN native_type TEXT;
                    ALTER TABLE sync_outbox ADD COLUMN envelope_schema_version INTEGER NOT NULL DEFAULT 1;
                    PRAGMA user_version = 15;")?;
                renew_legacy_sync_outbox(&transaction)?;
                transaction.commit()?;
            }
            13 => connection.execute_batch(MIGRATION_V14)?,
            14 => connection.execute_batch(MIGRATION_V15)?,
            15 => connection.execute_batch(MIGRATION_V16)?,
            16 => {
                let transaction = connection.transaction()?;
                transaction.execute_batch(MIGRATION_V17)?;
                // Every pre-v17 visible item was either local/private or
                // explicitly pinned locally; inbound shared clips were not applied.
                transaction.execute("INSERT OR IGNORE INTO shared_clip_bindings (share_id, remote_clip_id, local_clip_id, private_origin)
                    SELECT s.id, pi.clip_id, pi.clip_id, 1 FROM cloudkit_shares s
                    JOIN pinboard_items pi ON pi.pinboard_id = s.pinboard_id WHERE s.state = 'active'", [])?;
                share_materialize::freeze_pending_wire(&transaction)?;
                transaction.execute("INSERT OR IGNORE INTO shared_only_outbox (operation_id)
                    SELECT o.operation_id FROM sync_outbox o JOIN cloudkit_shares s
                    ON (o.entity_kind = 'pinboard' AND o.entity_id = s.pinboard_id)
                    OR (o.entity_kind = 'pinboard_membership' AND substr(o.entity_id, 1, instr(o.entity_id, ':') - 1) = s.pinboard_id)
                    WHERE s.role = 'participant'", [])?;
                transaction.commit()?;
            }
            17 => connection.execute_batch(MIGRATION_V18)?,
            18 => {
                let transaction = connection.transaction()?;
                transaction.execute_batch(MIGRATION_V19)?;
                transaction.commit()?;
            }
            19 => return Ok(()),
            other => return Err(StorageError::UnsupportedSchema(other)),
        }
    }
}

fn validate_backup_source(connection: &Connection) -> Result<(), StorageError> {
    let application_id: u32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id != APPLICATION_ID {
        return Err(StorageError::InvalidBackup(
            "file was not created by PasteRS".into(),
        ));
    }
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(MIN_BACKUP_SCHEMA_VERSION..=SCHEMA_VERSION).contains(&version) {
        return Err(StorageError::InvalidBackup(format!(
            "schema version {version} is not supported; expected {MIN_BACKUP_SCHEMA_VERSION} through {SCHEMA_VERSION}"
        )));
    }
    ensure_integrity(connection)
}

fn validate_current_database(connection: &Connection) -> Result<(), StorageError> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(StorageError::InvalidBackup(format!(
            "backup migration produced schema version {version}; expected {SCHEMA_VERSION}"
        )));
    }
    ensure_integrity(connection)
}

fn ensure_integrity(connection: &Connection) -> Result<(), StorageError> {
    let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(StorageError::InvalidBackup(format!(
            "SQLite integrity check failed: {result}"
        )))
    }
}

fn find_existing_clip(
    transaction: &Transaction<'_>,
    hash: &[u8; 32],
    content_kind: ContentKind,
) -> Result<Option<String>, StorageError> {
    transaction
        .query_row(
            "SELECT id FROM clips
             WHERE content_hash = ?1 AND content_kind = ?2 AND deleted_at_ms IS NULL
             ORDER BY EXISTS(SELECT 1 FROM shared_clip_bindings b WHERE b.local_clip_id = clips.id AND b.private_origin = 0), last_copied_at_ms DESC LIMIT 1",
            params![hash.as_slice(), content_kind.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn apply_retention_transaction(
    transaction: &Transaction<'_>,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<usize, StorageError> {
    let mut ids = BTreeSet::<String>::new();
    if let Some(days) = policy.max_age_days {
        let cutoff = now - Duration::days(i64::from(days));
        collect_ids(
            transaction,
            "SELECT c.id FROM clips c
             WHERE c.deleted_at_ms IS NULL AND c.last_copied_at_ms < ?1
               AND NOT EXISTS (SELECT 1 FROM pinboard_items pi WHERE pi.clip_id = c.id)
               AND NOT EXISTS (SELECT 1 FROM sync_deferred_memberships d WHERE d.clip_id = c.id)
               AND NOT EXISTS (SELECT 1 FROM shared_clip_bindings b WHERE b.local_clip_id = c.id AND b.private_origin = 0)",
            params![cutoff.timestamp_millis()], &mut ids,
        )?;
    }
    if let Some(limit) = policy.max_unpinned_items {
        collect_ids(
            transaction,
            "SELECT c.id FROM clips c
             WHERE c.deleted_at_ms IS NULL
               AND NOT EXISTS (SELECT 1 FROM pinboard_items pi WHERE pi.clip_id = c.id)
               AND NOT EXISTS (SELECT 1 FROM sync_deferred_memberships d WHERE d.clip_id = c.id)
               AND NOT EXISTS (SELECT 1 FROM shared_clip_bindings b WHERE b.local_clip_id = c.id AND b.private_origin = 0)
             ORDER BY c.last_copied_at_ms DESC
             LIMIT -1 OFFSET ?1",
            params![limit], &mut ids,
        )?;
    }
    for id in &ids {
        let tombstone = enqueue_sync_change(
            transaction,
            SyncEntityKind::Clip,
            id,
            SyncChangeKind::Delete,
        )?;
        // Retention supersedes queued saves, including their attachments.
        // Pinboard/deferred/shared-only protections above remain unchanged.
        supersede_clip_outbox(transaction, id, tombstone.operation_id)?;
        transaction.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
    }
    delete_unreferenced_blobs(transaction)?;
    Ok(ids.len())
}

fn insert_capture_transaction(
    transaction: &Transaction<'_>,
    capture: &CapturedItem,
) -> Result<ClipItem, StorageError> {
    insert_capture_transaction_with_kind(transaction, capture, capture.infer_content_kind())
}

fn insert_capture_transaction_with_kind(
    transaction: &Transaction<'_>,
    capture: &CapturedItem,
    content_kind: ContentKind,
) -> Result<ClipItem, StorageError> {
    let aggregate_hash = aggregate_hash(capture);
    if let Some(existing_id) = find_existing_clip(transaction, &aggregate_hash, content_kind)? {
        if share_materialize::isolated_origin(transaction, &existing_id)?.is_some() {
            return load_clip(transaction, &existing_id);
        }
        match ensure_clip_share_writable(transaction, &existing_id) {
            Err(StorageError::ReadOnlyPinboardShare) => {
                // Recopying shared read-only content must not edit its author,
                // device metadata or version. Explicit edits still fail below.
                return load_clip(transaction, &existing_id);
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        transaction.execute(
            "UPDATE clips SET last_copied_at_ms = ?1, source_bundle_id = ?2, \
             source_display_name = ?3, device_id = ?4, device_display_name = ?5 \
             WHERE id = ?6",
            params![
                capture.captured_at.timestamp_millis(),
                capture.source.bundle_identifier,
                capture.source.display_name,
                capture.device.id.to_string(),
                capture.device.display_name,
                existing_id,
            ],
        )?;
        enqueue_sync_change(
            transaction,
            SyncEntityKind::Clip,
            &existing_id,
            SyncChangeKind::Save,
        )?;
        return load_clip(transaction, &existing_id);
    }

    let id = ClipId::new();
    let searchable_text = capture
        .primary_decoded_text()
        .unwrap_or_default()
        .into_owned();
    let pdf_file_name = (content_kind == ContentKind::Pdf)
        .then(|| {
            capture
                .representations
                .iter()
                .filter(|representation| representation.kind == RepresentationKind::Pdf)
                .filter_map(|representation| representation.file_name.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .find_map(|name| std::path::Path::new(name).file_name()?.to_str())
        })
        .flatten();
    let title = pdf_file_name.map_or_else(
        || suggested_title(&searchable_text, content_kind),
        |name| truncate_chars(name, TITLE_LIMIT),
    );
    let now_ms = capture.captured_at.timestamp_millis();

    transaction.execute(
        "INSERT INTO clips (
            id, captured_at_ms, last_copied_at_ms, source_bundle_id,
            source_display_name, device_id, device_display_name, content_kind,
            title, searchable_text, content_hash
         ) VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id.to_string(),
            now_ms,
            capture.source.bundle_identifier,
            capture.source.display_name,
            capture.device.id.to_string(),
            capture.device.display_name,
            content_kind.as_str(),
            title,
            searchable_text,
            aggregate_hash.as_slice(),
        ],
    )?;

    insert_representations(
        transaction,
        &id.to_string(),
        &capture.representations,
        now_ms,
    )?;

    enqueue_sync_change(
        transaction,
        SyncEntityKind::Clip,
        &id.to_string(),
        SyncChangeKind::Save,
    )?;
    load_clip(transaction, &id.to_string())
}

fn insert_representations(
    transaction: &Transaction<'_>,
    clip_id: &str,
    representations: &[CapturedRepresentation],
    now_ms: i64,
) -> Result<(), StorageError> {
    for (ordinal, representation) in representations.iter().enumerate() {
        representation.validate()?;
        let representation_hash = *blake3::hash(&representation.bytes).as_bytes();
        transaction.execute(
            "INSERT OR IGNORE INTO blobs (content_hash, byte_len, bytes, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                representation_hash.as_slice(),
                to_i64(representation.bytes.len())?,
                representation.bytes,
                now_ms,
            ],
        )?;
        let text_preview = representation
            .decoded_text()
            .map(|value| truncate_chars(&value, TEXT_PREVIEW_LIMIT));
        transaction.execute(
            "INSERT INTO representations (
                clip_id, ordinal, representation_kind, mime_type, file_name,
                byte_len, content_hash, text_preview, native_type
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                clip_id,
                to_i64(ordinal)?,
                representation.kind.storage_key(),
                representation.mime_type,
                representation.file_name,
                to_i64(representation.bytes.len())?,
                representation_hash.as_slice(),
                text_preview,
                representation.native_type,
            ],
        )?;
    }
    Ok(())
}

fn delete_unreferenced_blobs(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let eligibility = if share_materialize::schema_ready(transaction)? {
        "o.state = 'acknowledged' OR EXISTS(SELECT 1 FROM shared_only_outbox x WHERE x.operation_id = o.operation_id)"
    } else {
        "o.state = 'acknowledged'"
    };
    transaction.execute(
        &format!(
            "DELETE FROM sync_outbox_snapshots
         WHERE operation_id IN (
             SELECT o.operation_id FROM sync_outbox o
             WHERE ({eligibility}) AND NOT EXISTS (
                 SELECT 1 FROM cloudkit_share_outbox so
                 WHERE so.operation_id = o.operation_id AND so.state = 'pending'
             )
         )"
        ),
        [],
    )?;
    transaction.execute(
        "DELETE FROM blobs
         WHERE NOT EXISTS (
             SELECT 1 FROM representations r WHERE r.content_hash = blobs.content_hash
         ) AND NOT EXISTS (
             SELECT 1 FROM sync_outbox_blob_refs r WHERE r.content_hash = blobs.content_hash
         )",
        [],
    )?;
    Ok(())
}

fn supersede_clip_outbox(
    transaction: &Transaction<'_>,
    clip_id: &str,
    replacement: uuid::Uuid,
) -> Result<(), StorageError> {
    let now = Utc::now().timestamp_millis();
    transaction.execute(
        "UPDATE cloudkit_share_outbox SET state = 'acknowledged', acknowledged_at_ms = ?1
         WHERE state = 'pending' AND operation_id IN (
             SELECT operation_id FROM sync_outbox
             WHERE entity_kind = 'clip' AND entity_id = ?2 AND operation_id != ?3
         )",
        params![now, clip_id, replacement.to_string()],
    )?;
    transaction.execute(
        "UPDATE sync_outbox SET state = 'acknowledged', acknowledged_at_ms = ?1
         WHERE state = 'pending' AND entity_kind = 'clip' AND entity_id = ?2
           AND operation_id != ?3",
        params![now, clip_id, replacement.to_string()],
    )?;
    Ok(())
}

fn enqueue_sync_change(
    transaction: &Transaction<'_>,
    entity_kind: SyncEntityKind,
    entity_id: &str,
    change_kind: SyncChangeKind,
) -> Result<SyncChange, StorageError> {
    if entity_kind == SyncEntityKind::Clip {
        ensure_clip_share_writable(transaction, entity_id)?;
    }
    let device_id = local_device_id(transaction)?;
    let stored_clock = transaction
        .query_row(
            "SELECT wall_time_ms, counter FROM sync_clocks WHERE device_id = ?1",
            params![device_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let mut clock = match stored_clock {
        Some((wall_time_ms, counter)) => HybridClock::from_timestamp(HybridTimestamp {
            wall_time_ms,
            counter: u32::try_from(counter).map_err(|error| corrupt("sync clock", error))?,
            node_id: device_id,
        }),
        None => HybridClock::new(device_id),
    };
    let timestamp = clock.tick(Utc::now().timestamp_millis());
    transaction.execute(
        "INSERT INTO sync_clocks (device_id, wall_time_ms, counter)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(device_id) DO UPDATE SET
             wall_time_ms = excluded.wall_time_ms,
             counter = excluded.counter",
        params![
            device_id.to_string(),
            timestamp.wall_time_ms,
            i64::from(timestamp.counter)
        ],
    )?;

    let entity_kind_key = sync_entity_kind_key(entity_kind);
    let stored_version = transaction
        .query_row(
            "SELECT version_json FROM sync_entity_versions
             WHERE entity_kind = ?1 AND entity_id = ?2",
            params![entity_kind_key, entity_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut version = stored_version.map_or_else(
        || Ok(VersionVector::default()),
        |json| serde_json::from_str(&json).map_err(|error| corrupt("sync entity version", error)),
    )?;
    version.increment(device_id);
    let version_json =
        serde_json::to_string(&version).map_err(|error| corrupt("sync entity version", error))?;
    let change = SyncChange::new(
        SyncEntity {
            kind: entity_kind,
            id: entity_id.to_owned(),
        },
        change_kind,
        timestamp,
        version,
    );
    let change_json =
        serde_json::to_string(&change).map_err(|error| corrupt("sync change", error))?;
    transaction.execute(
        "INSERT INTO sync_entity_versions (entity_kind, entity_id, version_json, change_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
             version_json = excluded.version_json,
             change_json = excluded.change_json",
        params![entity_kind_key, entity_id, version_json, &change_json],
    )?;
    transaction.execute(
        "INSERT INTO sync_outbox (
             operation_id, entity_kind, entity_id, change_kind,
             change_json, state, created_at_ms, logical_counter, node_id, envelope_schema_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9)",
        params![
            change.operation_id.to_string(),
            entity_kind_key,
            entity_id,
            sync_change_kind_key(change_kind),
            change_json,
            change.timestamp.wall_time_ms,
            i64::from(change.timestamp.counter),
            change.timestamp.node_id.to_string(),
            paste_sync::SYNC_ENVELOPE_SCHEMA_VERSION,
        ],
    )?;
    snapshot_sync_change(transaction, &change)?;
    if entity_kind == SyncEntityKind::PinboardMembership {
        // A later local pin/unpin/reorder supersedes a deferred remote relation.
        clear_deferred_membership(transaction, entity_id)?;
    }
    route_sync_change_to_shares(transaction, &change)?;
    if share_materialize::schema_ready(transaction)? {
        share_materialize::record_local_change(transaction, &change)?;
    }
    Ok(change)
}

fn snapshot_sync_change(
    transaction: &Transaction<'_>,
    change: &SyncChange,
) -> Result<(), StorageError> {
    let mut blobs = BTreeMap::new();
    let payload = match change.change {
        SyncChangeKind::Delete => None,
        SyncChangeKind::Save => Some(load_sync_payload(transaction, &change.entity, &mut blobs)?),
    };
    SyncEnvelope::new(SyncScope::Private, change.clone(), payload.clone())
        .map_err(|error| corrupt("sync snapshot", error))?;
    let payload_json =
        serde_json::to_string(&payload).map_err(|error| corrupt("sync snapshot", error))?;
    transaction.execute(
        "INSERT INTO sync_outbox_snapshots (operation_id, payload_json) VALUES (?1, ?2)",
        params![change.operation_id.to_string(), payload_json],
    )?;
    for hash in blobs.keys() {
        transaction.execute(
            "INSERT INTO sync_outbox_blob_refs (operation_id, content_hash) VALUES (?1, ?2)",
            params![change.operation_id.to_string(), hash.as_slice()],
        )?;
    }
    Ok(())
}

fn load_sync_snapshot(
    connection: &Connection,
    change: &SyncChange,
    blobs: &mut BTreeMap<[u8; 32], Vec<u8>>,
) -> Result<(u16, Option<SyncPayload>), StorageError> {
    let (json, schema_version): (String, u16) = connection.query_row(
        "SELECT s.payload_json, o.envelope_schema_version FROM sync_outbox_snapshots s
         JOIN sync_outbox o ON o.operation_id = s.operation_id WHERE s.operation_id = ?1",
        params![change.operation_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let payload: Option<SyncPayload> =
        serde_json::from_str(&json).map_err(|error| corrupt("stored sync snapshot", error))?;
    if let Some(SyncPayload::Clip(clip)) = &payload {
        for representation in &clip.representations {
            if blobs.contains_key(&representation.content_hash) {
                continue;
            }
            let bytes: Vec<u8> = connection.query_row(
                "SELECT b.bytes FROM blobs b
                 JOIN sync_outbox_blob_refs r ON r.content_hash = b.content_hash
                 WHERE r.operation_id = ?1 AND r.content_hash = ?2",
                params![
                    change.operation_id.to_string(),
                    representation.content_hash.as_slice()
                ],
                |row| row.get(0),
            )?;
            if *blake3::hash(&bytes).as_bytes() != representation.content_hash
                || bytes.len() as u64 != representation.byte_len
            {
                return Err(corrupt("stored sync snapshot", "invalid attachment"));
            }
            blobs.insert(representation.content_hash, bytes);
        }
    }
    Ok((schema_version, payload))
}

fn renew_legacy_sync_outbox(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let changes = {
        let mut statement = transaction.prepare(
            "SELECT DISTINCT o.change_json FROM sync_outbox o
             WHERE o.state = 'pending' OR EXISTS (
                 SELECT 1 FROM cloudkit_share_outbox so
                 WHERE so.operation_id = o.operation_id AND so.state = 'pending'
             )",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut entities = BTreeMap::new();
    for json in changes {
        let change: SyncChange =
            serde_json::from_str(&json).map_err(|error| corrupt("legacy sync change", error))?;
        entities.insert(
            (
                sync_entity_kind_key(change.entity.kind),
                change.entity.id.clone(),
            ),
            change.entity,
        );
    }
    // No legacy operation is uploaded again with guessed content under its old UUID.
    // Renewing and acknowledging them is atomic with the schema migration.
    transaction.execute(
        "UPDATE sync_outbox SET state = 'acknowledged', acknowledged_at_ms = ?1
         WHERE state = 'pending'",
        params![Utc::now().timestamp_millis()],
    )?;
    transaction.execute(
        "UPDATE cloudkit_share_outbox SET state = 'acknowledged', acknowledged_at_ms = ?1
         WHERE state = 'pending'",
        params![Utc::now().timestamp_millis()],
    )?;
    for entity in entities.into_values() {
        let kind = match load_sync_payload(transaction, &entity, &mut BTreeMap::new()) {
            Ok(_) => SyncChangeKind::Save,
            Err(StorageError::NotFound) => SyncChangeKind::Delete,
            Err(error) => return Err(error),
        };
        enqueue_sync_change(transaction, entity.kind, &entity.id, kind)?;
    }
    Ok(())
}

fn route_sync_change_to_shares(
    transaction: &Transaction<'_>,
    change: &SyncChange,
) -> Result<(), StorageError> {
    let operation_id = change.operation_id.to_string();
    let now_ms = Utc::now().timestamp_millis();
    match change.entity.kind {
        SyncEntityKind::Pinboard => {
            transaction.execute(
                "INSERT OR IGNORE INTO cloudkit_share_outbox (
                     share_id, operation_id, state, created_at_ms
                 )
                 SELECT id, ?1, 'pending', ?2 FROM cloudkit_shares
                 WHERE pinboard_id = ?3 AND state = 'active'
                   AND (role = 'owner' OR permission = 'read_write')",
                params![operation_id, now_ms, &change.entity.id],
            )?;
        }
        SyncEntityKind::PinboardMembership => {
            let (pinboard_id, clip_id) = change
                .entity
                .id
                .split_once(':')
                .ok_or_else(|| corrupt("pinboard membership entity id", "missing separator"))?;
            let routed = transaction.execute(
                "INSERT OR IGNORE INTO cloudkit_share_outbox (
                     share_id, operation_id, state, created_at_ms
                 )
                 SELECT id, ?1, 'pending', ?2 FROM cloudkit_shares
                 WHERE pinboard_id = ?3 AND state = 'active'
                   AND (role = 'owner' OR permission = 'read_write')",
                params![operation_id, now_ms, pinboard_id],
            )?;
            if routed > 0 && change.change == SyncChangeKind::Save {
                // Imported clips and already-acknowledged local clips may not
                // have a retained outbox snapshot. Create a new immutable
                // operation rather than reusing a UUID with reconstructed data.
                let has_snapshot: bool = transaction.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sync_entity_versions ev
                         JOIN sync_outbox o ON o.change_json = ev.change_json
                         JOIN sync_outbox_snapshots ss ON ss.operation_id = o.operation_id
                         WHERE ev.entity_kind = 'clip' AND ev.entity_id = ?1
                     )",
                    params![clip_id],
                    |row| row.get(0),
                )?;
                if !has_snapshot {
                    enqueue_sync_change(
                        transaction,
                        SyncEntityKind::Clip,
                        clip_id,
                        SyncChangeKind::Save,
                    )?;
                }
                transaction.execute(
                    "INSERT OR IGNORE INTO cloudkit_share_outbox (
                         share_id, operation_id, state, created_at_ms
                     )
                     SELECT s.id, o.operation_id, 'pending', ?1
                     FROM cloudkit_shares s
                     JOIN sync_entity_versions ev
                       ON ev.entity_kind = 'clip' AND ev.entity_id = ?3
                     JOIN sync_outbox o
                       ON o.entity_kind = ev.entity_kind
                      AND o.entity_id = ev.entity_id
                      AND o.change_json = ev.change_json
                     WHERE s.pinboard_id = ?2 AND s.state = 'active'
                       AND (s.role = 'owner' OR s.permission = 'read_write')",
                    params![now_ms, pinboard_id, clip_id],
                )?;
            }
        }
        SyncEntityKind::Clip => {
            transaction.execute(
                "INSERT OR IGNORE INTO cloudkit_share_outbox (
                     share_id, operation_id, state, created_at_ms
                 )
                 SELECT s.id, ?1, 'pending', ?2
                 FROM cloudkit_shares s
                 JOIN pinboard_items pi ON pi.pinboard_id = s.pinboard_id
                 WHERE pi.clip_id = ?3 AND s.state = 'active'
                   AND (s.role = 'owner' OR s.permission = 'read_write')",
                params![operation_id, now_ms, &change.entity.id],
            )?;
        }
    }
    Ok(())
}

fn seed_pinboard_share_outbox(
    transaction: &Transaction<'_>,
    pinboard_id: PinboardId,
) -> Result<(), StorageError> {
    let pinboard_id = pinboard_id.to_string();
    let clip_ids = {
        let mut statement = transaction.prepare(
            "SELECT pi.clip_id FROM pinboard_items pi JOIN clips c ON c.id = pi.clip_id
             WHERE pi.pinboard_id = ?1 AND c.deleted_at_ms IS NULL ORDER BY pi.position",
        )?;
        statement
            .query_map(params![pinboard_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for clip_id in clip_ids {
        enqueue_sync_change(
            transaction,
            SyncEntityKind::Clip,
            &clip_id,
            SyncChangeKind::Save,
        )?;
        enqueue_sync_change(
            transaction,
            SyncEntityKind::PinboardMembership,
            &format!("{pinboard_id}:{clip_id}"),
            SyncChangeKind::Save,
        )?;
    }
    Ok(())
}

fn local_device_id(transaction: &Transaction<'_>) -> Result<DeviceId, StorageError> {
    let stored = transaction
        .query_row(
            "SELECT value FROM settings WHERE key = 'device_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match stored {
        Some(value) => {
            DeviceId::from_str(&value).map_err(|error| corrupt("stored device id", error))
        }
        None => {
            let device_id = DeviceId::new();
            transaction.execute(
                "INSERT INTO settings (key, value, updated_at_ms)
                 VALUES ('device_id', ?1, ?2)",
                params![device_id.to_string(), Utc::now().timestamp_millis()],
            )?;
            Ok(device_id)
        }
    }
}

const fn sync_entity_kind_key(kind: SyncEntityKind) -> &'static str {
    match kind {
        SyncEntityKind::Clip => "clip",
        SyncEntityKind::Pinboard => "pinboard",
        SyncEntityKind::PinboardMembership => "pinboard_membership",
    }
}

const fn sync_change_kind_key(kind: SyncChangeKind) -> &'static str {
    match kind {
        SyncChangeKind::Save => "save",
        SyncChangeKind::Delete => "delete",
    }
}

fn validate_sync_scope(scope: &str) -> Result<(), StorageError> {
    if matches!(scope, "private" | "shared") {
        Ok(())
    } else {
        Err(StorageError::InvalidSyncScope)
    }
}

fn pinboard_membership_id(pinboard_id: PinboardId, clip_id: ClipId) -> String {
    format!("{pinboard_id}:{clip_id}")
}

fn load_sync_entity_change(
    connection: &Connection,
    entity: &SyncEntity,
) -> Result<Option<SyncChange>, StorageError> {
    let raw = connection
        .query_row(
            "SELECT change_json FROM sync_entity_versions
             WHERE entity_kind = ?1 AND entity_id = ?2",
            params![sync_entity_kind_key(entity.kind), &entity.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;
    match raw {
        None => Ok(None),
        Some(Some(json)) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| corrupt("stored sync change metadata", error)),
        Some(None) => Err(StorageError::CorruptData(
            "sync entity version is missing change metadata".into(),
        )),
    }
}

fn store_remote_sync_change(
    transaction: &Transaction<'_>,
    change: &SyncChange,
) -> Result<(), StorageError> {
    let version_json = serde_json::to_string(&change.version)
        .map_err(|error| corrupt("remote sync version", error))?;
    let change_json =
        serde_json::to_string(change).map_err(|error| corrupt("remote sync change", error))?;
    transaction.execute(
        "INSERT INTO sync_entity_versions (entity_kind, entity_id, version_json, change_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
             version_json = excluded.version_json,
             change_json = excluded.change_json",
        params![
            sync_entity_kind_key(change.entity.kind),
            &change.entity.id,
            version_json,
            change_json
        ],
    )?;
    Ok(())
}

fn store_sync_conflict(
    transaction: &Transaction<'_>,
    conflict: &RemoteConflict,
    incoming_blobs: &BTreeMap<[u8; 32], &[u8]>,
) -> Result<(), StorageError> {
    let local_json = serde_json::to_string(&conflict.local)
        .map_err(|error| corrupt("local sync conflict", error))?;
    let remote_json = serde_json::to_string(&conflict.remote)
        .map_err(|error| corrupt("remote sync conflict", error))?;
    let id = conflict.remote.change.operation_id.to_string();
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO sync_conflicts (
             id, entity_kind, entity_id, local_change_json, remote_envelope_json,
             remote_would_win, created_at_ms, resolved_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
        params![
            &id,
            sync_entity_kind_key(conflict.remote.change.entity.kind),
            &conflict.remote.change.entity.id,
            &local_json,
            &remote_json,
            i64::from(conflict.decision.disposition == RemoteDisposition::Apply),
            Utc::now().timestamp_millis()
        ],
    )?;
    if changed == 0 {
        let stored: String = transaction.query_row(
            "SELECT remote_envelope_json FROM sync_conflicts WHERE id = ?1",
            params![&id],
            |row| row.get(0),
        )?;
        if stored != remote_json {
            return Err(StorageError::CorruptData(
                "one remote operation ID contains conflicting envelopes".into(),
            ));
        }
    }
    if let Some(SyncPayload::Clip(item)) = conflict.remote.payload.as_ref() {
        for representation in &item.representations {
            let bytes = match incoming_blobs.get(&representation.content_hash) {
                Some(bytes) => bytes.to_vec(),
                None => transaction
                    .query_row(
                        "SELECT bytes FROM blobs WHERE content_hash = ?1",
                        params![representation.content_hash.as_slice()],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()?
                    .ok_or_else(|| {
                        StorageError::CorruptData(
                            "remote conflict is missing a referenced blob".into(),
                        )
                    })?,
            };
            if *blake3::hash(&bytes).as_bytes() != representation.content_hash
                || u64::try_from(bytes.len())
                    .map_err(|error| corrupt("sync conflict blob length", error))?
                    != representation.byte_len
            {
                return Err(StorageError::CorruptData(
                    "remote conflict blob does not match metadata".into(),
                ));
            }
            transaction.execute(
                "INSERT OR REPLACE INTO sync_conflict_blobs (
                     conflict_id, content_hash, byte_len, bytes
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    &id,
                    representation.content_hash.as_slice(),
                    to_i64(bytes.len())?,
                    bytes
                ],
            )?;
        }
    }
    Ok(())
}

fn load_sync_conflict_blobs(
    transaction: &Transaction<'_>,
    conflict_id: uuid::Uuid,
) -> Result<BTreeMap<[u8; 32], Vec<u8>>, StorageError> {
    let mut statement = transaction.prepare(
        "SELECT content_hash, byte_len, bytes
         FROM sync_conflict_blobs WHERE conflict_id = ?1",
    )?;
    let raw = statement
        .query_map(params![conflict_id.to_string()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(hash, byte_len, bytes)| {
            if byte_len < 0
                || usize::try_from(byte_len)
                    .map_err(|error| corrupt("sync conflict blob length", error))?
                    != bytes.len()
            {
                return Err(StorageError::CorruptData(
                    "stored sync conflict blob has an invalid length".into(),
                ));
            }
            let hash = fixed_hash(hash, "sync conflict content hash")?;
            if *blake3::hash(&bytes).as_bytes() != hash {
                return Err(StorageError::CorruptData(
                    "stored sync conflict blob has an invalid hash".into(),
                ));
            }
            Ok((hash, bytes))
        })
        .collect()
}

fn observe_remote_sync_clock(
    transaction: &Transaction<'_>,
    remote: &HybridTimestamp,
) -> Result<(), StorageError> {
    let device_id = local_device_id(transaction)?;
    let stored = transaction
        .query_row(
            "SELECT wall_time_ms, counter FROM sync_clocks WHERE device_id = ?1",
            params![device_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let mut clock = match stored {
        Some((wall_time_ms, counter)) => HybridClock::from_timestamp(HybridTimestamp {
            wall_time_ms,
            counter: u32::try_from(counter).map_err(|error| corrupt("sync clock", error))?,
            node_id: device_id,
        }),
        None => HybridClock::new(device_id),
    };
    let observed = clock.observe(remote, Utc::now().timestamp_millis());
    transaction.execute(
        "INSERT INTO sync_clocks (device_id, wall_time_ms, counter)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(device_id) DO UPDATE SET
             wall_time_ms = excluded.wall_time_ms,
             counter = excluded.counter",
        params![
            device_id.to_string(),
            observed.wall_time_ms,
            i64::from(observed.counter)
        ],
    )?;
    Ok(())
}

fn membership_dependencies_ready(
    connection: &Connection,
    membership: &PinboardMembershipSnapshot,
) -> Result<bool, StorageError> {
    if membership.position < 0 {
        return Err(StorageError::InvalidRemoteSyncBatch);
    }
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pinboards WHERE id = ?1)
             AND EXISTS(SELECT 1 FROM clips WHERE id = ?2 AND deleted_at_ms IS NULL)",
        params![
            membership.pinboard_id.to_string(),
            membership.clip_id.to_string()
        ],
        |row| row.get(0),
    )?)
}

fn deferred_membership_count(connection: &Connection) -> Result<usize, StorageError> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sync_deferred_memberships",
        [],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|error| corrupt("deferred membership count", error))
}

fn defer_remote_membership(
    transaction: &Transaction<'_>,
    envelope: &SyncEnvelope,
    membership: &PinboardMembershipSnapshot,
) -> Result<(), StorageError> {
    let json =
        serde_json::to_string(envelope).map_err(|error| corrupt("deferred membership", error))?;
    transaction.execute(
        "INSERT INTO sync_deferred_memberships (entity_id, pinboard_id, clip_id, envelope_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(entity_id) DO UPDATE SET envelope_json = excluded.envelope_json",
        params![
            &envelope.change.entity.id,
            membership.pinboard_id.to_string(),
            membership.clip_id.to_string(),
            json
        ],
    )?;
    if deferred_membership_count(transaction)? > DEFERRED_MEMBERSHIP_LIMIT {
        return Err(StorageError::DeferredSyncQueueFull);
    }
    Ok(())
}

fn clear_deferred_membership(
    transaction: &Transaction<'_>,
    entity_id: &str,
) -> Result<(), StorageError> {
    transaction.execute(
        "DELETE FROM sync_deferred_memberships WHERE entity_id = ?1",
        params![entity_id],
    )?;
    Ok(())
}

fn drain_deferred_memberships(transaction: &Transaction<'_>) -> Result<usize, StorageError> {
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT d.envelope_json FROM sync_deferred_memberships d
             JOIN pinboards p ON p.id = d.pinboard_id
             JOIN clips c ON c.id = d.clip_id AND c.deleted_at_ms IS NULL
             ORDER BY d.entity_id LIMIT ?1",
        )?;
        statement
            .query_map(params![DEFERRED_MEMBERSHIP_DRAIN_LIMIT as u32], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut applied = 0;
    for json in rows {
        let envelope: SyncEnvelope = serde_json::from_str(&json)
            .map_err(|error| corrupt("stored deferred membership", error))?;
        envelope
            .validate()
            .map_err(|error| corrupt("stored deferred membership", error))?;
        if envelope.scope != SyncScope::Private
            || !matches!(envelope.payload, Some(SyncPayload::PinboardMembership(_)))
        {
            return Err(StorageError::InvalidRemoteSyncBatch);
        }
        let current = load_sync_entity_change(transaction, &envelope.change.entity)?;
        if current.is_some_and(|current| current.operation_id == envelope.change.operation_id) {
            apply_remote_envelope(transaction, &envelope, &BTreeMap::new())?;
            applied += 1;
        }
        clear_deferred_membership(transaction, &envelope.change.entity.id)?;
    }
    Ok(applied)
}

fn apply_remote_envelope(
    transaction: &Transaction<'_>,
    envelope: &SyncEnvelope,
    blobs: &BTreeMap<[u8; 32], &[u8]>,
) -> Result<(), StorageError> {
    match (&envelope.change.change, envelope.payload.as_ref()) {
        (SyncChangeKind::Save, Some(SyncPayload::Clip(item))) => {
            upsert_remote_clip(transaction, item, blobs)
        }
        (SyncChangeKind::Save, Some(SyncPayload::Pinboard(item))) => {
            let name = item.name.trim();
            if name.is_empty() || name.chars().count() > TITLE_LIMIT {
                return Err(StorageError::InvalidPinboardName);
            }
            let color = normalize_pinboard_color(&item.color)?;
            transaction.execute(
                "INSERT INTO pinboards (
                     id, name, color, sort_order, created_at_ms, updated_at_ms, is_shared
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     color = excluded.color,
                     sort_order = excluded.sort_order,
                     created_at_ms = excluded.created_at_ms,
                     updated_at_ms = excluded.updated_at_ms,
                     is_shared = excluded.is_shared",
                params![
                    item.id.to_string(),
                    name,
                    color,
                    item.sort_order,
                    item.created_at.timestamp_millis(),
                    item.updated_at.timestamp_millis(),
                    item.is_shared
                ],
            )?;
            Ok(())
        }
        (SyncChangeKind::Save, Some(SyncPayload::PinboardMembership(item))) => {
            if item.position < 0 {
                return Err(StorageError::InvalidRemoteSyncBatch);
            }
            ensure_exists(transaction, "pinboards", &item.pinboard_id.to_string())?;
            ensure_exists(transaction, "clips", &item.clip_id.to_string())?;
            transaction.execute(
                "INSERT INTO pinboard_items (pinboard_id, clip_id, position, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(pinboard_id, clip_id) DO UPDATE SET
                     position = excluded.position,
                     created_at_ms = excluded.created_at_ms",
                params![
                    item.pinboard_id.to_string(),
                    item.clip_id.to_string(),
                    item.position,
                    item.created_at.timestamp_millis()
                ],
            )?;
            Ok(())
        }
        (SyncChangeKind::Delete, None) => apply_remote_delete(transaction, &envelope.change.entity),
        _ => Err(StorageError::CorruptData(
            "validated sync envelope has an inconsistent payload".into(),
        )),
    }
}

fn remote_apply_priority(envelope: &SyncEnvelope) -> u8 {
    match (envelope.change.change, envelope.change.entity.kind) {
        (SyncChangeKind::Save, SyncEntityKind::Clip) => 0,
        (SyncChangeKind::Save, SyncEntityKind::Pinboard) => 1,
        (SyncChangeKind::Save, SyncEntityKind::PinboardMembership) => 2,
        (SyncChangeKind::Delete, SyncEntityKind::PinboardMembership) => 3,
        (SyncChangeKind::Delete, SyncEntityKind::Clip) => 4,
        (SyncChangeKind::Delete, SyncEntityKind::Pinboard) => 5,
    }
}

fn upsert_remote_clip(
    transaction: &Transaction<'_>,
    item: &ClipItem,
    blobs: &BTreeMap<[u8; 32], &[u8]>,
) -> Result<(), StorageError> {
    if item.title.trim().is_empty()
        || item.title.chars().count() > TITLE_LIMIT
        || item.searchable_text.len() > TEXTUAL_EDIT_LIMIT_BYTES
    {
        return Err(StorageError::InvalidRemoteSyncBatch);
    }
    let mut representations = Vec::with_capacity(item.representations.len());
    for metadata in &item.representations {
        let bytes = match blobs.get(&metadata.content_hash) {
            Some(bytes) => bytes.to_vec(),
            None => transaction
                .query_row(
                    "SELECT bytes FROM blobs WHERE content_hash = ?1",
                    params![metadata.content_hash.as_slice()],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    StorageError::CorruptData(format!(
                        "remote clip {} is missing a referenced blob",
                        item.id
                    ))
                })?,
        };
        if u64::try_from(bytes.len()).map_err(|error| corrupt("remote blob length", error))?
            != metadata.byte_len
            || *blake3::hash(&bytes).as_bytes() != metadata.content_hash
        {
            return Err(StorageError::CorruptData(
                "remote clip blob metadata does not match its bytes".into(),
            ));
        }
        representations.push(CapturedRepresentation {
            native_type: metadata.native_type.clone(),
            kind: metadata.kind.clone(),
            mime_type: metadata.mime_type.clone(),
            file_name: metadata.file_name.clone(),
            bytes,
        });
    }
    if representations.is_empty()
        || aggregate_representation_hash(&representations) != item.content_hash
    {
        return Err(StorageError::CorruptData(
            "remote clip aggregate hash does not match its representations".into(),
        ));
    }

    let id = item.id.to_string();
    transaction.execute(
        "INSERT INTO clips (
             id, captured_at_ms, last_copied_at_ms, source_bundle_id,
             source_display_name, device_id, device_display_name, content_kind,
             title, searchable_text, content_hash, deleted_at_ms, delete_batch_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL)
         ON CONFLICT(id) DO UPDATE SET
             captured_at_ms = excluded.captured_at_ms,
             last_copied_at_ms = excluded.last_copied_at_ms,
             source_bundle_id = excluded.source_bundle_id,
             source_display_name = excluded.source_display_name,
             device_id = excluded.device_id,
             device_display_name = excluded.device_display_name,
             content_kind = excluded.content_kind,
             title = excluded.title,
             searchable_text = excluded.searchable_text,
             content_hash = excluded.content_hash,
             deleted_at_ms = NULL,
             delete_batch_id = NULL",
        params![
            &id,
            item.captured_at.timestamp_millis(),
            item.last_copied_at.timestamp_millis(),
            &item.source.bundle_identifier,
            &item.source.display_name,
            item.device.id.to_string(),
            &item.device.display_name,
            item.content_kind.as_str(),
            &item.title,
            &item.searchable_text,
            item.content_hash.as_slice()
        ],
    )?;
    transaction.execute(
        "DELETE FROM representations WHERE clip_id = ?1",
        params![&id],
    )?;
    transaction.execute("DELETE FROM preview_cache WHERE clip_id = ?1", params![&id])?;
    insert_representations(
        transaction,
        &id,
        &representations,
        Utc::now().timestamp_millis(),
    )?;
    Ok(())
}

fn apply_remote_delete(
    transaction: &Transaction<'_>,
    entity: &SyncEntity,
) -> Result<(), StorageError> {
    match entity.kind {
        SyncEntityKind::Clip => {
            transaction.execute(
                "UPDATE clips SET deleted_at_ms = ?1, delete_batch_id = NULL
                 WHERE id = ?2 AND deleted_at_ms IS NULL",
                params![Utc::now().timestamp_millis(), &entity.id],
            )?;
        }
        SyncEntityKind::Pinboard => {
            transaction.execute("DELETE FROM pinboards WHERE id = ?1", params![&entity.id])?;
        }
        SyncEntityKind::PinboardMembership => {
            let (pinboard_id, clip_id) = entity
                .id
                .split_once(':')
                .ok_or_else(|| StorageError::CorruptData("invalid membership id".into()))?;
            transaction.execute(
                "DELETE FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2",
                params![pinboard_id, clip_id],
            )?;
        }
    }
    Ok(())
}

fn load_sync_payload(
    connection: &Connection,
    entity: &SyncEntity,
    blobs: &mut BTreeMap<[u8; 32], Vec<u8>>,
) -> Result<SyncPayload, StorageError> {
    match entity.kind {
        SyncEntityKind::Clip => {
            let item = load_clip(connection, &entity.id)?;
            let representations = load_clipboard_payload_from_connection(connection, &entity.id)?;
            if representations.len() != item.representations.len() {
                return Err(StorageError::CorruptData(
                    "clip representation metadata and payload count differ".into(),
                ));
            }
            for (metadata, representation) in item.representations.iter().zip(representations) {
                let content_hash = *blake3::hash(&representation.bytes).as_bytes();
                if content_hash != metadata.content_hash {
                    return Err(StorageError::CorruptData(
                        "clip representation blob hash does not match metadata".into(),
                    ));
                }
                blobs.entry(content_hash).or_insert(representation.bytes);
            }
            Ok(SyncPayload::Clip(item))
        }
        SyncEntityKind::Pinboard => {
            let raw = connection
                .query_row(
                    "SELECT p.id, p.name, p.color, p.sort_order, p.created_at_ms,
                            p.updated_at_ms, p.is_shared, COUNT(c.id)
                     FROM pinboards p
                     LEFT JOIN pinboard_items pi ON pi.pinboard_id = p.id
                     LEFT JOIN clips c ON c.id = pi.clip_id AND c.deleted_at_ms IS NULL
                     WHERE p.id = ?1
                     GROUP BY p.id",
                    params![&entity.id],
                    |row| {
                        Ok(RawPinboard {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            color: row.get(2)?,
                            sort_order: row.get(3)?,
                            created_at_ms: row.get(4)?,
                            updated_at_ms: row.get(5)?,
                            is_shared: row.get(6)?,
                            item_count: row.get(7)?,
                        })
                    },
                )
                .optional()?
                .ok_or(StorageError::NotFound)?;
            Ok(SyncPayload::Pinboard(raw_pinboard(raw)?))
        }
        SyncEntityKind::PinboardMembership => {
            let (pinboard_id, clip_id) = entity
                .id
                .split_once(':')
                .ok_or_else(|| StorageError::CorruptData("invalid membership id".into()))?;
            let pinboard_id = PinboardId::from_str(pinboard_id)
                .map_err(|error| corrupt("membership pinboard id", error))?;
            let clip_id =
                ClipId::from_str(clip_id).map_err(|error| corrupt("membership clip id", error))?;
            let (position, created_at_ms) = connection
                .query_row(
                    "SELECT position, created_at_ms FROM pinboard_items
                     WHERE pinboard_id = ?1 AND clip_id = ?2",
                    params![pinboard_id.to_string(), clip_id.to_string()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?
                .ok_or(StorageError::NotFound)?;
            Ok(SyncPayload::PinboardMembership(
                PinboardMembershipSnapshot {
                    pinboard_id,
                    clip_id,
                    position,
                    created_at: timestamp(created_at_ms)?,
                },
            ))
        }
    }
}

fn load_clipboard_payload_from_connection(
    connection: &Connection,
    id: &str,
) -> Result<Vec<CapturedRepresentation>, StorageError> {
    ensure_exists(connection, "clips", id)?;
    let mut statement = connection.prepare(
        "SELECT r.representation_kind, r.mime_type, r.file_name, b.bytes, r.native_type
         FROM representations r
         JOIN blobs b ON b.content_hash = r.content_hash
         WHERE r.clip_id = ?1
         ORDER BY r.ordinal",
    )?;
    let values = statement
        .query_map(params![id], |row| {
            let kind = row.get::<_, String>(0)?;
            Ok(CapturedRepresentation {
                native_type: row.get(4)?,
                kind: RepresentationKind::from_storage_key(&kind),
                mime_type: row.get(1)?,
                file_name: row.get(2)?,
                bytes: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        Err(StorageError::CorruptData(
            "clip does not contain stored representations".into(),
        ))
    } else {
        Ok(values)
    }
}

fn load_clip(connection: &Connection, id: &str) -> Result<ClipItem, StorageError> {
    let raw = connection
        .prepare_cached(
            "SELECT id, captured_at_ms, last_copied_at_ms, source_bundle_id,
                    source_display_name, device_id, device_display_name,
                    content_kind, title, searchable_text, content_hash
             FROM clips WHERE id = ?1 AND deleted_at_ms IS NULL",
        )?
        .query_row(params![id], |row| {
            Ok(RawClip {
                id: row.get(0)?,
                captured_at_ms: row.get(1)?,
                last_copied_at_ms: row.get(2)?,
                source_bundle_id: row.get(3)?,
                source_display_name: row.get(4)?,
                device_id: row.get(5)?,
                device_display_name: row.get(6)?,
                content_kind: row.get(7)?,
                title: row.get(8)?,
                searchable_text: row.get(9)?,
                content_hash: row.get(10)?,
            })
        })
        .optional()?
        .ok_or(StorageError::NotFound)?;

    let mut statement = connection.prepare_cached(
        "SELECT representation_kind, mime_type, file_name, byte_len,
                content_hash, text_preview, native_type
         FROM representations WHERE clip_id = ?1 ORDER BY ordinal",
    )?;
    let representations = statement
        .query_map(params![id], |row| {
            Ok(RawRepresentation {
                kind: row.get(0)?,
                mime_type: row.get(1)?,
                file_name: row.get(2)?,
                byte_len: row.get(3)?,
                content_hash: row.get(4)?,
                text_preview: row.get(5)?,
                native_type: row.get(6)?,
            })
        })?
        .map(|row| raw_representation(row?))
        .collect::<Result<Vec<_>, StorageError>>()?;

    Ok(ClipItem {
        id: ClipId::from_str(&raw.id).map_err(|error| corrupt("clip id", error))?,
        captured_at: timestamp(raw.captured_at_ms)?,
        last_copied_at: timestamp(raw.last_copied_at_ms)?,
        source: SourceApplication {
            bundle_identifier: raw.source_bundle_id,
            display_name: raw.source_display_name,
        },
        device: DeviceMetadata {
            id: DeviceId::from_str(&raw.device_id).map_err(|error| corrupt("device id", error))?,
            display_name: raw.device_display_name,
        },
        content_kind: ContentKind::from_str(&raw.content_kind)?,
        title: raw.title,
        searchable_text: raw.searchable_text,
        content_hash: fixed_hash(raw.content_hash, "clip hash")?,
        representations,
    })
}

fn raw_representation(raw: RawRepresentation) -> Result<PersistedRepresentation, StorageError> {
    Ok(PersistedRepresentation {
        native_type: raw.native_type,
        kind: RepresentationKind::from_storage_key(&raw.kind),
        mime_type: raw.mime_type,
        file_name: raw.file_name,
        byte_len: u64::try_from(raw.byte_len).map_err(|error| corrupt("byte length", error))?,
        content_hash: fixed_hash(raw.content_hash, "representation hash")?,
        text_preview: raw.text_preview,
    })
}

fn raw_pinboard(raw: RawPinboard) -> Result<Pinboard, StorageError> {
    Ok(Pinboard {
        id: PinboardId::from_str(&raw.id).map_err(|error| corrupt("pinboard id", error))?,
        name: raw.name,
        color: raw.color,
        sort_order: raw.sort_order,
        created_at: timestamp(raw.created_at_ms)?,
        updated_at: timestamp(raw.updated_at_ms)?,
        is_shared: raw.is_shared,
        item_count: u64::try_from(raw.item_count).map_err(|error| corrupt("item count", error))?,
    })
}

fn load_pinboard_share(
    transaction: &Transaction<'_>,
    pinboard_id: PinboardId,
) -> Result<Option<PinboardShare>, StorageError> {
    transaction
        .query_row(
            "SELECT id, pinboard_id, zone_name, owner_name, share_record_name, share_url,
                    role, permission, state, server_change_token, created_at_ms, updated_at_ms
             FROM cloudkit_shares WHERE pinboard_id = ?1",
            params![pinboard_id.to_string()],
            raw_pinboard_share_row,
        )
        .optional()?
        .map(parse_pinboard_share)
        .transpose()
}

fn raw_pinboard_share_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPinboardShare> {
    Ok(RawPinboardShare {
        id: row.get(0)?,
        pinboard_id: row.get(1)?,
        zone_name: row.get(2)?,
        owner_name: row.get(3)?,
        share_record_name: row.get(4)?,
        share_url: row.get(5)?,
        role: row.get(6)?,
        permission: row.get(7)?,
        state: row.get(8)?,
        server_change_token: row.get(9)?,
        created_at_ms: row.get(10)?,
        updated_at_ms: row.get(11)?,
    })
}

fn parse_pinboard_share(raw: RawPinboardShare) -> Result<PinboardShare, StorageError> {
    let role = match raw.role.as_str() {
        "owner" => PinboardShareRole::Owner,
        "participant" => PinboardShareRole::Participant,
        _ => return Err(StorageError::InvalidCloudKitShareMetadata),
    };
    let permission = match raw.permission.as_str() {
        "read_only" => PinboardSharePermission::ReadOnly,
        "read_write" => PinboardSharePermission::ReadWrite,
        _ => return Err(StorageError::InvalidCloudKitShareMetadata),
    };
    let state = match raw.state.as_str() {
        "preparing" => PinboardShareState::Preparing,
        "active" => PinboardShareState::Active,
        "revoked" => PinboardShareState::Revoked,
        _ => return Err(StorageError::InvalidCloudKitShareMetadata),
    };
    Ok(PinboardShare {
        id: uuid::Uuid::parse_str(&raw.id).map_err(|error| corrupt("CloudKit share id", error))?,
        pinboard_id: raw
            .pinboard_id
            .map(|id| {
                PinboardId::from_str(&id).map_err(|error| corrupt("shared pinboard id", error))
            })
            .transpose()?,
        zone_name: raw.zone_name,
        owner_name: raw.owner_name,
        share_record_name: raw.share_record_name,
        share_url: raw.share_url,
        role,
        permission,
        state,
        server_change_token: raw.server_change_token,
        created_at: timestamp(raw.created_at_ms)?,
        updated_at: timestamp(raw.updated_at_ms)?,
    })
}

fn pinboard_share_permission_key(permission: PinboardSharePermission) -> &'static str {
    match permission {
        PinboardSharePermission::ReadOnly => "read_only",
        PinboardSharePermission::ReadWrite => "read_write",
    }
}

fn validate_cloudkit_record_component(value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StorageError::InvalidCloudKitShareMetadata);
    }
    Ok(())
}

fn validate_share_url(value: &str) -> Result<(), StorageError> {
    let url = url::Url::parse(value).map_err(|_| StorageError::InvalidCloudKitShareMetadata)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(StorageError::InvalidCloudKitShareMetadata);
    }
    Ok(())
}

fn normalize_pinboard_color(value: &str) -> Result<String, StorageError> {
    let value = value.trim();
    if value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(StorageError::InvalidPinboardColor)
    }
}

fn aggregate_hash(capture: &CapturedItem) -> [u8; 32] {
    aggregate_representation_hash(&capture.representations)
}

fn aggregate_representation_hash(representations: &[CapturedRepresentation]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for representation in representations {
        // Leave the legacy hash byte-for-byte identical when metadata is
        // absent. Typed records use a domain separator and explicit length.
        if let Some(native_type) = &representation.native_type {
            hasher.update(b"\x00PasteRS-native-type-v1\x00");
            hasher.update(&(native_type.len() as u64).to_le_bytes());
            hasher.update(native_type.as_bytes());
        }
        hasher.update(representation.kind.storage_key().as_bytes());
        hasher.update(&[0]);
        hasher.update(&representation.bytes);
        hasher.update(&[0xff]);
    }
    *hasher.finalize().as_bytes()
}

fn rich_text_editable(item: &ClipItem) -> bool {
    matches!(item.content_kind, ContentKind::Text | ContentKind::RichText | ContentKind::Html)
        || item.representations.iter().any(|representation| {
            matches!(representation.kind, RepresentationKind::Rtf | RepresentationKind::Html)
                || representation.native_type.as_deref() == Some("com.apple.flat-rtfd")
                || matches!(&representation.kind, RepresentationKind::Custom(kind) if kind == "com.apple.flat-rtfd")
        })
}

fn normalize_textual_edit(kind: ContentKind, value: &str) -> Result<String, StorageError> {
    if value.len() > TEXTUAL_EDIT_LIMIT_BYTES {
        return Err(StorageError::TextualEditTooLarge);
    }
    match kind {
        ContentKind::Text => {
            if value.trim().is_empty() {
                Err(StorageError::EmptyTextualEdit)
            } else {
                Ok(value.to_owned())
            }
        }
        ContentKind::Link => {
            let value = value.trim();
            if value.is_empty()
                || value.contains(char::is_whitespace)
                || url::Url::parse(value).is_err()
            {
                Err(StorageError::InvalidEditedUrl)
            } else {
                Ok(value.to_owned())
            }
        }
        ContentKind::Color => {
            let [r, g, b] =
                paste_domain::parse_color_code(value).ok_or(StorageError::InvalidEditedColor)?;
            Ok(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        _ => Err(StorageError::UnsupportedTextualEditKind),
    }
}

fn textual_representations(
    kind: ContentKind,
    value: &str,
) -> Result<Vec<CapturedRepresentation>, StorageError> {
    match kind {
        ContentKind::Text => Ok(vec![CapturedRepresentation::plain_text(value)]),
        ContentKind::Link => Ok(vec![
            CapturedRepresentation {
                native_type: None,
                kind: RepresentationKind::Url,
                mime_type: Some("text/uri-list; charset=utf-8".into()),
                file_name: None,
                bytes: value.as_bytes().to_vec(),
            },
            CapturedRepresentation::plain_text(value),
        ]),
        ContentKind::Color => Ok(vec![CapturedRepresentation::plain_text(value)]),
        _ => Err(StorageError::UnsupportedTextualEditKind),
    }
}

fn suggested_title(text: &str, content_kind: ContentKind) -> String {
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        format!("{} item", content_kind.as_str().replace('_', " "))
    } else {
        truncate_chars(first_line.trim(), TITLE_LIMIT)
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() && limit > 0 {
        // The ellipsis is part of the limit, including for Unicode titles.
        result.pop();
        result.push('…');
    }
    result
}

fn fts_expression(text: &str) -> String {
    text.split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn append_in_filter(
    sql: &mut String,
    values: &mut Vec<Value>,
    column: &str,
    source: impl Iterator<Item = String>,
) {
    let items = source.collect::<Vec<_>>();
    if items.is_empty() {
        return;
    }
    sql.push_str(&format!(
        " AND {column} IN ({})",
        repeat_placeholders(items.len())
    ));
    values.extend(items.into_iter().map(Value::Text));
}

fn repeat_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn collect_ids<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
    output: &mut BTreeSet<String>,
) -> Result<(), StorageError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, |row| row.get::<_, String>(0))?;
    for row in rows {
        output.insert(row?);
    }
    Ok(())
}

fn ensure_exists(connection: &Connection, table: &str, id: &str) -> Result<(), StorageError> {
    let sql = match table {
        "clips" => "SELECT 1 FROM clips WHERE id = ?1 AND deleted_at_ms IS NULL",
        "pinboards" => "SELECT 1 FROM pinboards WHERE id = ?1",
        _ => return Err(StorageError::InvalidTable),
    };
    let exists = connection
        .query_row(sql, params![id], |_| Ok(()))
        .optional()?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(StorageError::NotFound)
    }
}

fn ensure_pinboard_share_writable(
    connection: &Connection,
    pinboard_id: PinboardId,
) -> Result<(), StorageError> {
    let read_only = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM cloudkit_shares
             WHERE pinboard_id = ?1 AND state = 'active'
               AND role = 'participant' AND permission = 'read_only'
         )",
        params![pinboard_id.to_string()],
        |row| row.get::<_, bool>(0),
    )?;
    if read_only {
        Err(StorageError::ReadOnlyPinboardShare)
    } else {
        Ok(())
    }
}

fn load_pinboard_clip_order(
    connection: &Connection,
    pinboard_id: PinboardId,
) -> Result<Vec<ClipId>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT pi.clip_id FROM pinboard_items pi JOIN clips c ON c.id = pi.clip_id
         WHERE pi.pinboard_id = ?1 AND c.deleted_at_ms IS NULL
         ORDER BY pi.position ASC, c.last_copied_at_ms DESC, c.id DESC",
    )?;
    statement
        .query_map(params![pinboard_id.to_string()], |row| {
            row.get::<_, String>(0)
        })?
        .map(|row| ClipId::from_str(&row?).map_err(|error| corrupt("pinboard clip id", error)))
        .collect()
}

/// Pinning is a move, not a copy. Existing databases may still contain old
/// multi-board memberships; normalize only the explicitly selected clips.
fn pin_clips_transaction(
    transaction: &Transaction<'_>,
    pinboard_id: PinboardId,
    clip_ids: &[ClipId],
) -> Result<bool, StorageError> {
    if clip_ids.len() > 200 {
        return Err(StorageError::InvalidPinboardPlacement);
    }
    ensure_exists(transaction, "pinboards", &pinboard_id.to_string())?;
    ensure_pinboard_share_writable(transaction, pinboard_id)?;
    let mut changed = false;
    for clip_id in clip_ids {
        ensure_exists(transaction, "clips", &clip_id.to_string())?;
        ensure_clip_share_writable(transaction, &clip_id.to_string())?;
        if let Some(origin) = share_materialize::isolated_origin(transaction, &clip_id.to_string())?
        {
            let own_board: String = transaction.query_row(
                "SELECT pinboard_id FROM cloudkit_shares WHERE id = ?1",
                params![origin],
                |row| row.get(0),
            )?;
            if own_board != pinboard_id.to_string() {
                return Err(StorageError::SharedCrossScopeMove);
            }
        }
        let sources = {
            let mut statement = transaction.prepare(
                "SELECT pinboard_id FROM pinboard_items WHERE clip_id = ?1 AND pinboard_id != ?2",
            )?;
            statement
                .query_map(
                    params![clip_id.to_string(), pinboard_id.to_string()],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        for source in sources {
            let source = PinboardId::from_str(&source)
                .map_err(|error| corrupt("source pinboard id", error))?;
            ensure_pinboard_share_writable(transaction, source)?;
            transaction.execute(
                "DELETE FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2",
                params![source.to_string(), clip_id.to_string()],
            )?;
            enqueue_sync_change(
                transaction,
                SyncEntityKind::PinboardMembership,
                &pinboard_membership_id(source, *clip_id),
                SyncChangeKind::Delete,
            )?;
            changed = true;
        }
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO pinboard_items (pinboard_id, clip_id, position, created_at_ms)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM pinboard_items WHERE pinboard_id = ?1), ?3)",
            params![pinboard_id.to_string(), clip_id.to_string(), Utc::now().timestamp_millis()],
        )?;
        if inserted > 0 {
            enqueue_sync_change(
                transaction,
                SyncEntityKind::PinboardMembership,
                &pinboard_membership_id(pinboard_id, *clip_id),
                SyncChangeKind::Save,
            )?;
            changed = true;
        }
    }
    Ok(changed)
}

fn ensure_clip_share_writable(connection: &Connection, clip_id: &str) -> Result<(), StorageError> {
    if let Some(origin) = share_materialize::isolated_origin(connection, clip_id)? {
        let writable: bool = connection.query_row("SELECT state = 'active' AND (role = 'owner' OR permission = 'read_write') FROM cloudkit_shares WHERE id = ?1", params![origin], |row| row.get(0))?;
        if !writable {
            return Err(StorageError::ReadOnlyPinboardShare);
        }
    }
    let read_only: bool = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pinboard_items pi
             JOIN cloudkit_shares s ON s.pinboard_id = pi.pinboard_id
             WHERE pi.clip_id = ?1 AND s.state = 'active'
               AND s.role = 'participant' AND s.permission = 'read_only'
         )",
        params![clip_id],
        |row| row.get(0),
    )?;
    if read_only {
        Err(StorageError::ReadOnlyPinboardShare)
    } else {
        Ok(())
    }
}

fn timestamp(milliseconds: i64) -> Result<DateTime<Utc>, StorageError> {
    Utc.timestamp_millis_opt(milliseconds)
        .single()
        .ok_or_else(|| StorageError::CorruptData("invalid timestamp".into()))
}

fn fixed_hash(bytes: Vec<u8>, field: &str) -> Result<[u8; 32], StorageError> {
    bytes
        .try_into()
        .map_err(|_| StorageError::CorruptData(format!("{field} has an invalid length")))
}

fn to_i64(value: usize) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|error| corrupt("integer conversion", error))
}

fn corrupt(context: &str, error: impl std::fmt::Display) -> StorageError {
    StorageError::CorruptData(format!("{context}: {error}"))
}

struct RawClip {
    id: String,
    captured_at_ms: i64,
    last_copied_at_ms: i64,
    source_bundle_id: String,
    source_display_name: String,
    device_id: String,
    device_display_name: String,
    content_kind: String,
    title: String,
    searchable_text: String,
    content_hash: Vec<u8>,
}

struct RawRepresentation {
    kind: String,
    native_type: Option<String>,
    mime_type: Option<String>,
    file_name: Option<String>,
    byte_len: i64,
    content_hash: Vec<u8>,
    text_preview: Option<String>,
}

struct RawPinboard {
    id: String,
    name: String,
    color: String,
    sort_order: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
    is_shared: bool,
    item_count: i64,
}

struct RawPinboardShare {
    id: String,
    pinboard_id: Option<String>,
    zone_name: String,
    owner_name: Option<String>,
    share_record_name: Option<String>,
    share_url: Option<String>,
    role: String,
    permission: String,
    state: String,
    server_change_token: Option<Vec<u8>>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Domain(#[from] paste_domain::DomainError),
    #[error("capture was rejected by the privacy policy")]
    CaptureRejected,
    #[error("requested item does not exist")]
    NotFound,
    #[error("too many incoming Pinboard relations are waiting for their clips or boards")]
    DeferredSyncQueueFull,
    #[error("pinboard name cannot be empty")]
    InvalidPinboardName,
    #[error("pinboard color must be a #RRGGBB value")]
    InvalidPinboardColor,
    #[error("pinboard order must contain every pinboard exactly once")]
    InvalidPinboardOrder,
    #[error("pinboard item direction must be -1 or 1")]
    InvalidPinboardItemMove,
    #[error("pinboard placement must contain between 1 and 200 distinct clips")]
    InvalidPinboardPlacement,
    #[error("this Pinboard already belongs to another CloudKit share")]
    PinboardAlreadyShared,
    #[error("CloudKit share metadata is invalid")]
    InvalidCloudKitShareMetadata,
    #[error("this shared Pinboard is read-only on the current device")]
    ReadOnlyPinboardShare,
    #[error("textual content cannot be empty")]
    EmptyTextualEdit,
    #[error("textual content exceeds the 4 MiB edit limit")]
    TextualEditTooLarge,
    #[error("this content kind cannot be edited as text")]
    UnsupportedTextualEditKind,
    #[error("富文本格式无效，原内容未修改。")]
    InvalidRichTextEdit,
    #[error("内容已被其他操作或同步更新；请保留当前草稿，重新打开最新内容后再编辑。")]
    StaleContentEdit,
    #[error("edited link is not a valid URL")]
    InvalidEditedUrl,
    #[error("edited color must be a #RRGGBB value")]
    InvalidEditedColor,
    #[error("image edit must be a valid bounded PNG representation")]
    InvalidImageEdit,
    #[error("no text was recognized in the image")]
    EmptyOcrText,
    #[error("CloudKit sync batches must contain between 1 and 250 changes")]
    InvalidSyncBatchLimit,
    #[error("sync acknowledgement refers to an unknown operation")]
    UnknownSyncOperation,
    #[error("shared sync acknowledgement refers to an unknown pending delivery")]
    UnknownSharedSyncOperation,
    #[error("sync scope must be private or shared")]
    InvalidSyncScope,
    #[error("sync change token is empty or exceeds 4 MiB")]
    InvalidSyncToken,
    #[error("remote sync batch exceeds the 250-operation or 2048-blob limit")]
    InvalidRemoteSyncBatch,
    #[error("shared download identity, permission or checkpoint changed; fetch again")]
    StaleSharedDownload,
    #[error("shared operation UUID was reused with different content")]
    SharedOperationMismatch,
    #[error("shared download inbox reached its bounded pending capacity")]
    SharedInboxFull,
    #[error(
        "moving imported shared content to another scope requires an explicit independent copy"
    )]
    SharedCrossScopeMove,
    #[error("invalid shared conflict identifier")]
    InvalidSharedConflictId,
    #[error("共享内容在预览后已改变，请刷新后重新选择；未覆盖任何内容。")]
    StaleSharedConflict,
    #[error("remote sync payload exceeds the 64 MiB per-blob or 256 MiB batch limit")]
    RemoteSyncPayloadTooLarge,
    #[error("sync conflict list limit must be between 1 and 100")]
    InvalidSyncConflictLimit,
    #[error("MCP client name cannot be empty or exceed 80 characters")]
    InvalidMcpClientName,
    #[error("database schema version {0} is newer than this application")]
    UnsupportedSchema(u32),
    #[error("database contains invalid data: {0}")]
    CorruptData(String),
    #[error("database lock was poisoned")]
    Poisoned,
    #[error("internal table selector is invalid")]
    InvalidTable,
    #[error("capture batch unexpectedly produced no records")]
    EmptyBatch,
    #[error("backup destination path is invalid")]
    InvalidBackupPath,
    #[error("backup file is invalid: {0}")]
    InvalidBackup(String),
}

#[cfg(test)]
mod text_limit_tests {
    use super::truncate_chars;

    #[test]
    fn truncation_includes_ellipsis_in_the_unicode_character_limit() {
        for (value, limit, expected) in [
            ("", 0, ""),
            ("😀界", 0, ""),
            ("😀", 1, "😀"),
            ("😀界", 1, "…"),
            ("😀界", 2, "😀界"),
            ("😀界A", 2, "😀…"),
            ("😀界", 3, "😀界"),
        ] {
            assert_eq!(truncate_chars(value, limit), expected);
        }
        for count in [79, 80, 81, 1_000] {
            let output = truncate_chars(&"😀".repeat(count), 80);
            assert_eq!(output.chars().count(), count.min(80));
            assert_eq!(output.ends_with('…'), count > 80);
        }
    }
}
