//! Durable transport boundary. This inbox is deliberately not private history:
//! a separate share-scoped materializer must reconcile entities and permissions
//! before exposing them or publishing any local edits. Receiving != applying.
use super::*;

const MAX_ENVELOPE_BYTES: usize = 768 * 1024;
const MAX_PENDING_OPERATIONS: usize = 100_000;
const MAX_PENDING_BYTES: i64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SharedInboxReport {
    pub received: usize,
    pub duplicates: usize,
    pub pending: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SharedInboxBatch {
    pub envelopes: Vec<SyncEnvelope>,
    pub blobs: Vec<SyncBlob>,
}

impl SqliteStore {
    /// Commits bytes, immutable remote operations and the checkpoint together.
    /// `expected` is the share snapshot used to start the network request; its
    /// identity and token must still match. No local history/outbox is changed.
    pub fn stage_pinboard_share_sync_page(
        &self,
        expected: &PinboardShare,
        envelopes: &[SyncEnvelope],
        blobs: &[SyncBlob],
        server_change_token: &[u8],
    ) -> Result<SharedInboxReport, StorageError> {
        if server_change_token.is_empty() || server_change_token.len() > 4 * 1024 * 1024 {
            return Err(StorageError::InvalidSyncToken);
        }
        if envelopes.len() > 250 || blobs.len() > REMOTE_SYNC_BATCH_BLOB_LIMIT {
            return Err(StorageError::InvalidRemoteSyncBatch);
        }
        let board = expected
            .pinboard_id
            .ok_or(StorageError::InvalidCloudKitShareMetadata)?;
        let mut json = Vec::with_capacity(envelopes.len());
        let mut incoming = BTreeMap::new();
        let mut total = 0usize;
        for blob in blobs {
            total = total
                .checked_add(blob.bytes.len())
                .ok_or(StorageError::RemoteSyncPayloadTooLarge)?;
            if blob.bytes.len() > REMOTE_SYNC_BLOB_LIMIT_BYTES
                || total > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES
            {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            if *blake3::hash(&blob.bytes).as_bytes() != blob.content_hash {
                return Err(corrupt("shared download blob", "hash mismatch"));
            }
            incoming.insert(blob.content_hash, blob.bytes.as_slice());
        }
        for envelope in envelopes {
            validate_share_envelope(envelope, board)?;
            let encoded = serde_json::to_string(envelope)
                .map_err(|error| corrupt("shared envelope", error))?;
            if encoded.len() > MAX_ENVELOPE_BYTES {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            total = total
                .checked_add(encoded.len())
                .ok_or(StorageError::RemoteSyncPayloadTooLarge)?;
            if total > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            json.push(encoded);
        }

        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let current = load_pinboard_share(&transaction, board)?.ok_or(StorageError::NotFound)?;
        validate_checkpoint(&current, expected)?;
        let share_id = expected.id.to_string();
        let mut report = SharedInboxReport::default();
        for (envelope, encoded) in envelopes.iter().zip(&json) {
            if super::share_materialize::check_receipt(&transaction, &share_id, envelope)? {
                report.duplicates += 1;
                continue;
            }
            let operation_id = envelope.change.operation_id.to_string();
            let existing: Option<String> = transaction.query_row(
                "SELECT envelope_json FROM cloudkit_share_inbox WHERE share_id = ?1 AND operation_id = ?2",
                params![&share_id, &operation_id], |row| row.get(0),
            ).optional()?;
            if let Some(existing) = existing {
                let existing: SyncEnvelope = serde_json::from_str(&existing)
                    .map_err(|error| corrupt("shared inbox", error))?;
                if &existing != envelope {
                    return Err(StorageError::SharedOperationMismatch);
                }
                report.duplicates += 1;
                continue;
            }
            // Validate every attachment and the aggregate hash before storing
            // an operation. Only this share's received blobs may be reused.
            let attachments = validate_attachments(&transaction, &share_id, envelope, &incoming)?;
            if attachments.values().map(Vec::len).sum::<usize>() + encoded.len()
                > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES
            {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            transaction.execute(
                "INSERT INTO cloudkit_share_inbox (share_id, operation_id, envelope_json, received_at_ms)
                 VALUES (?1, ?2, ?3, ?4)", params![&share_id, &operation_id, encoded, Utc::now().timestamp_millis()],
            )?;
            for (hash, bytes) in attachments {
                transaction.execute(
                    "INSERT OR IGNORE INTO cloudkit_share_inbox_blobs (share_id, content_hash, byte_len, bytes)
                     VALUES (?1, ?2, ?3, ?4)", params![&share_id, hash.as_slice(), to_i64(bytes.len())?, &bytes],
                )?;
                transaction.execute(
                    "INSERT OR IGNORE INTO cloudkit_share_inbox_blob_refs (share_id, operation_id, content_hash)
                     VALUES (?1, ?2, ?3)", params![&share_id, &operation_id, hash.as_slice()],
                )?;
            }
            report.received += 1;
        }
        report.pending = inbox_count(&transaction, Some(expected.id))?;
        // Backpressure until materialization can release received journal rows;
        // never drop an operation and advance its checkpoint to hide a full inbox.
        let bytes: i64 = transaction.query_row(
            "SELECT COALESCE((SELECT SUM(byte_len) FROM cloudkit_share_inbox_blobs), 0)
                  + COALESCE((SELECT SUM(length(CAST(envelope_json AS BLOB))) FROM cloudkit_share_inbox), 0)",
            [], |row| row.get(0),
        )?;
        if inbox_count(&transaction, None)? > MAX_PENDING_OPERATIONS || bytes > MAX_PENDING_BYTES {
            return Err(StorageError::SharedInboxFull);
        }
        transaction.execute(
            "UPDATE cloudkit_shares SET server_change_token = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![
                &share_id,
                server_change_token,
                Utc::now().timestamp_millis()
            ],
        )?;
        transaction.commit()?;
        Ok(report)
    }

    /// Token-expiry recovery uses the same compare-and-swap gate as a page.
    /// Received operations remain available for idempotent full-zone replay.
    pub fn reset_pinboard_share_download_checkpoint(
        &self,
        expected: &PinboardShare,
    ) -> Result<(), StorageError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let current = load_pinboard_share(
            &transaction,
            expected.pinboard_id.ok_or(StorageError::NotFound)?,
        )?
        .ok_or(StorageError::NotFound)?;
        validate_checkpoint(&current, expected)?;
        transaction.execute("UPDATE cloudkit_shares SET server_change_token = NULL, updated_at_ms = ?2 WHERE id = ?1",
            params![expected.id.to_string(), Utc::now().timestamp_millis()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_shared_download_count(&self) -> Result<usize, StorageError> {
        let connection = self.lock()?;
        inbox_count(&connection, None)
    }

    /// Bounded inspection for the future materializer. Reading does not consume
    /// rows or authorize marking their effects applied in local history.
    pub fn read_pinboard_share_inbox(
        &self,
        share_id: uuid::Uuid,
        limit: u32,
    ) -> Result<SharedInboxBatch, StorageError> {
        if !(1..=250).contains(&limit) {
            return Err(StorageError::InvalidSyncBatchLimit);
        }
        let connection = self.lock()?;
        let board: Option<String> = connection
            .query_row(
                "SELECT pinboard_id FROM cloudkit_shares WHERE id = ?1 AND state = 'active'",
                params![share_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let board = PinboardId::from_str(&board.ok_or(StorageError::NotFound)?)
            .map_err(|error| corrupt("share board", error))?;
        let mut statement = connection.prepare("SELECT envelope_json FROM cloudkit_share_inbox i WHERE share_id = ?1
            AND NOT EXISTS(SELECT 1 FROM shared_operation_receipts r WHERE r.share_id = i.share_id AND r.operation_id = i.operation_id)
            ORDER BY received_at_ms, operation_id LIMIT ?2")?;
        let rows = statement
            .query_map(params![share_id.to_string(), limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut batch = SharedInboxBatch::default();
        let mut blobs = BTreeMap::new();
        let mut total = 0usize;
        for json in rows {
            let envelope: SyncEnvelope =
                serde_json::from_str(&json).map_err(|error| corrupt("shared inbox", error))?;
            validate_share_envelope(&envelope, board)?;
            let attachments = validate_attachments(
                &connection,
                &share_id.to_string(),
                &envelope,
                &BTreeMap::new(),
            )?;
            let additional = attachments
                .iter()
                .filter(|(hash, _)| !blobs.contains_key(*hash))
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>();
            if total + additional + json.len() > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES {
                if batch.envelopes.is_empty() {
                    return Err(StorageError::RemoteSyncPayloadTooLarge);
                }
                break;
            }
            total += additional + json.len();
            blobs.extend(attachments);
            batch.envelopes.push(envelope);
        }
        batch.blobs = blobs
            .into_iter()
            .map(|(content_hash, bytes)| SyncBlob {
                content_hash,
                bytes,
            })
            .collect();
        Ok(batch)
    }
}

fn inbox_count(
    connection: &Connection,
    share_id: Option<uuid::Uuid>,
) -> Result<usize, StorageError> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM cloudkit_share_inbox i WHERE (?1 IS NULL OR share_id = ?1)
         AND NOT EXISTS(SELECT 1 FROM shared_operation_receipts r WHERE r.share_id = i.share_id AND r.operation_id = i.operation_id)",
        params![share_id.map(|id| id.to_string())],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|error| corrupt("shared inbox count", error))
}

fn validate_checkpoint(
    current: &PinboardShare,
    expected: &PinboardShare,
) -> Result<(), StorageError> {
    if current.state != PinboardShareState::Active
        || expected.state != PinboardShareState::Active
        || current.id != expected.id
        || current.pinboard_id != expected.pinboard_id
        || current.zone_name != expected.zone_name
        || current.owner_name != expected.owner_name
        || current.share_record_name != expected.share_record_name
        || current.role != expected.role
        || current.permission != expected.permission
        || current.server_change_token != expected.server_change_token
        || current.owner_name.is_none()
        || current.share_record_name.is_none()
        || current.zone_name
            != format!(
                "PasteShare_{}",
                current
                    .pinboard_id
                    .ok_or(StorageError::InvalidCloudKitShareMetadata)?
            )
    {
        return Err(StorageError::StaleSharedDownload);
    }
    Ok(())
}

pub(super) fn validate_share_envelope(
    envelope: &SyncEnvelope,
    board: PinboardId,
) -> Result<(), StorageError> {
    if envelope.scope != SyncScope::Shared {
        return Err(StorageError::InvalidRemoteSyncBatch);
    }
    envelope
        .validate()
        .map_err(|error| corrupt("shared envelope", error))?;
    let entity = &envelope.change.entity;
    match entity.kind {
        SyncEntityKind::Clip => {
            ClipId::from_str(&entity.id).map_err(|error| corrupt("shared clip ID", error))?;
        }
        SyncEntityKind::Pinboard => {
            if entity.id != board.to_string() {
                return Err(StorageError::InvalidCloudKitShareMetadata);
            }
        }
        SyncEntityKind::PinboardMembership => {
            let (board_id, clip_id) = entity
                .id
                .split_once(':')
                .ok_or(StorageError::InvalidRemoteSyncBatch)?;
            if board_id != board.to_string() || ClipId::from_str(clip_id).is_err() {
                return Err(StorageError::InvalidCloudKitShareMetadata);
            }
        }
    }
    match &envelope.payload {
        Some(SyncPayload::Clip(item))
            if item.title.trim().is_empty()
                || item.title.chars().count() > TITLE_LIMIT
                || item.searchable_text.len() > TEXTUAL_EDIT_LIMIT_BYTES
                || item.representations.is_empty()
                || item.representations.len() > 32 =>
        {
            return Err(StorageError::InvalidRemoteSyncBatch);
        }
        Some(SyncPayload::Pinboard(item)) => {
            if item.name.trim().is_empty() || item.name.chars().count() > TITLE_LIMIT {
                return Err(StorageError::InvalidPinboardName);
            }
            normalize_pinboard_color(&item.color)?;
        }
        Some(SyncPayload::PinboardMembership(item)) if item.position < 0 => {
            return Err(StorageError::InvalidRemoteSyncBatch);
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_attachments(
    connection: &Connection,
    share_id: &str,
    envelope: &SyncEnvelope,
    incoming: &BTreeMap<[u8; 32], &[u8]>,
) -> Result<BTreeMap<[u8; 32], Vec<u8>>, StorageError> {
    let mut result = BTreeMap::new();
    if let Some(SyncPayload::Clip(item)) = &envelope.payload {
        let mut representations = Vec::new();
        let mut total = 0usize;
        for metadata in &item.representations {
            if metadata.byte_len > REMOTE_SYNC_BLOB_LIMIT_BYTES as u64 {
                return Err(StorageError::RemoteSyncPayloadTooLarge);
            }
            let bytes = match incoming.get(&metadata.content_hash) {
                Some(bytes) => bytes.to_vec(),
                None => connection.query_row("SELECT bytes FROM cloudkit_share_inbox_blobs WHERE share_id = ?1 AND content_hash = ?2",
                    params![share_id, metadata.content_hash.as_slice()], |row| row.get::<_, Vec<u8>>(0)).optional()?.ok_or_else(|| corrupt("shared attachment", "missing in this share"))?,
            };
            total = total
                .checked_add(bytes.len())
                .ok_or(StorageError::RemoteSyncPayloadTooLarge)?;
            if total > REMOTE_SYNC_BATCH_BLOB_LIMIT_BYTES
                || bytes.len() as u64 != metadata.byte_len
                || *blake3::hash(&bytes).as_bytes() != metadata.content_hash
            {
                return Err(corrupt("shared attachment", "invalid size or hash"));
            }
            representations.push(CapturedRepresentation {
                kind: metadata.kind.clone(),
                native_type: metadata.native_type.clone(),
                mime_type: metadata.mime_type.clone(),
                file_name: metadata.file_name.clone(),
                bytes,
            });
        }
        if aggregate_representation_hash(&representations) != item.content_hash {
            return Err(corrupt("shared clip", "aggregate hash mismatch"));
        }
        for representation in representations {
            result.insert(
                *blake3::hash(&representation.bytes).as_bytes(),
                representation.bytes,
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pending_inbox_rolls_back_new_rows_and_does_not_advance_token() {
        let store = SqliteStore::open_in_memory().expect("isolated store");
        let board = store
            .create_pinboard("Capacity fixture", "#34c759")
            .expect("board");
        store
            .reserve_owned_pinboard_share(board.id, PinboardSharePermission::ReadWrite)
            .expect("reserve");
        let share = store
            .activate_owned_pinboard_share(
                board.id,
                "synthetic_owner",
                "zone_share",
                "https://www.icloud.com/share/synthetic",
            )
            .expect("active");
        let mut envelope = store
            .prepare_pinboard_share_sync_batch(share.id, 250)
            .expect("batch")
            .operations
            .into_iter()
            .find(|op| op.envelope.change.entity.kind == SyncEntityKind::Pinboard)
            .expect("board change")
            .envelope;
        envelope.change.change = SyncChangeKind::Delete;
        envelope.change.operation_id = uuid::Uuid::new_v4();
        envelope.payload = None;
        let encoded = serde_json::to_string(&envelope).expect("fixture JSON");
        {
            let connection = store.lock().expect("connection");
            // Fill a valid synthetic journal efficiently without a network or
            // 400 separate pages. Each row has its own matching operation UUID.
            connection.execute(
                "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1)
                 INSERT INTO cloudkit_share_inbox (share_id, operation_id, envelope_json, received_at_ms)
                 SELECT ?2, printf('%08x-0000-4000-8000-000000000000', n),
                    json_set(?3, '$.change.operation_id', printf('%08x-0000-4000-8000-000000000000', n)), 0 FROM seq",
                params![MAX_PENDING_OPERATIONS as i64, share.id.to_string(), encoded],
            ).expect("synthetic full journal");
        }
        assert!(matches!(
            store.stage_pinboard_share_sync_page(&share, &[envelope], &[], b"must-not-commit"),
            Err(StorageError::SharedInboxFull)
        ));
        assert_eq!(
            store.pending_shared_download_count().expect("capacity"),
            MAX_PENDING_OPERATIONS
        );
        assert!(
            store.list_pinboard_shares().expect("unchanged checkpoint")[0]
                .server_change_token
                .is_none()
        );
        assert_eq!(store.list_pinboards().expect("no materialization").len(), 1);
    }
}
