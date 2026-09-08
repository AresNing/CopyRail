//! Share-local causal state, immutable wire identities, and local projections.
//! A remote UUID can only select a binding in its authenticated share, never a
//! row in private history. Journal rows referenced by a winner/conflict survive
//! compaction; receipts retain the immutable-operation check after compaction.
use super::share_inbox::{validate_attachments, validate_share_envelope};
use super::*;

mod conflicts;
pub use conflicts::{
    SharedConflictId, SharedConflictResolution, SharedConflictSummary, SharedConflictVersion,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SharedMaterializationReport {
    pub consumed: usize,
    pub projected: usize,
    pub pending: usize,
    pub conflicts: usize,
}

pub(super) fn schema_ready(connection: &Connection) -> Result<bool, StorageError> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(version >= 17)
}

fn decode(json: &str) -> Result<SyncEnvelope, StorageError> {
    serde_json::from_str(json).map_err(|error| corrupt("shared state", error))
}

fn encode(envelope: &SyncEnvelope) -> Result<String, StorageError> {
    serde_json::to_string(envelope).map_err(|error| corrupt("shared state", error))
}

fn state(
    connection: &Connection,
    share: &str,
    kind: SyncEntityKind,
    id: &str,
) -> Result<Option<SyncEnvelope>, StorageError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT i.envelope_json FROM shared_entity_states s JOIN cloudkit_share_inbox i
         ON i.share_id = s.share_id AND i.operation_id = s.operation_id
         WHERE s.share_id = ?1 AND s.entity_kind = ?2 AND s.entity_id = ?3",
            params![share, sync_entity_kind_key(kind), id],
            |row| row.get(0),
        )
        .optional()?;
    json.as_deref().map(decode).transpose()
}

pub(super) fn check_receipt(
    connection: &Connection,
    share: &str,
    envelope: &SyncEnvelope,
) -> Result<bool, StorageError> {
    let hash: Option<Vec<u8>> = connection.query_row(
        "SELECT envelope_hash FROM shared_operation_receipts WHERE share_id = ?1 AND operation_id = ?2",
        params![share, envelope.change.operation_id.to_string()], |row| row.get(0),
    ).optional()?;
    match hash {
        Some(hash) if hash == blake3::hash(encode(envelope)?.as_bytes()).as_bytes() => Ok(true),
        Some(_) => Err(StorageError::SharedOperationMismatch),
        None => Ok(false),
    }
}

fn receipt(tx: &Transaction<'_>, share: &str, envelope: &SyncEnvelope) -> Result<(), StorageError> {
    check_receipt(tx, share, envelope)?;
    tx.execute("INSERT OR IGNORE INTO shared_operation_receipts (share_id, operation_id, envelope_hash) VALUES (?1, ?2, ?3)",
        params![share, envelope.change.operation_id.to_string(), blake3::hash(encode(envelope)?.as_bytes()).as_bytes().as_slice()])?;
    Ok(())
}

fn merge_state(
    tx: &Transaction<'_>,
    share: &str,
    envelope: &SyncEnvelope,
) -> Result<bool, StorageError> {
    let entity = &envelope.change.entity;
    let local = state(tx, share, entity.kind, &entity.id)?;
    let decision = decide_remote_change(local.as_ref().map(|e| &e.change), &envelope.change);
    if decision.preserve_losing_clip
        && let Some(local) = &local
    {
        tx.execute("INSERT OR IGNORE INTO shared_conflicts (share_id, local_operation_id, remote_operation_id) VALUES (?1, ?2, ?3)",
            params![share, local.change.operation_id.to_string(), envelope.change.operation_id.to_string()])?;
    }
    let changed = decision.disposition == RemoteDisposition::Apply;
    if changed {
        tx.execute("INSERT INTO shared_entity_states (share_id, entity_kind, entity_id, operation_id) VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(share_id, entity_kind, entity_id) DO UPDATE SET operation_id = excluded.operation_id",
            params![share, sync_entity_kind_key(entity.kind), &entity.id, envelope.change.operation_id.to_string()])?;
    }
    receipt(tx, share, envelope)?;
    if changed {
        conflicts::clear_dominated_conflicts(tx, share, &envelope.change)?;
    }
    Ok(changed)
}

fn binding(
    connection: &Connection,
    share: &str,
    remote: &str,
) -> Result<Option<(String, bool)>, StorageError> {
    connection.query_row("SELECT local_clip_id, private_origin FROM shared_clip_bindings WHERE share_id = ?1 AND remote_clip_id = ?2",
        params![share, remote], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(Into::into)
}

pub(super) fn isolated_origin(
    connection: &Connection,
    local: &str,
) -> Result<Option<String>, StorageError> {
    if !schema_ready(connection)? {
        return Ok(None);
    }
    connection.query_row("SELECT share_id FROM shared_clip_bindings WHERE local_clip_id = ?1 AND private_origin = 0",
        params![local], |row| row.get(0)).optional().map_err(Into::into)
}

fn bind_local(tx: &Transaction<'_>, share: &str, local: &str) -> Result<String, StorageError> {
    let remote: Option<String> = tx.query_row("SELECT remote_clip_id FROM shared_clip_bindings WHERE share_id = ?1 AND local_clip_id = ?2",
        params![share, local], |row| row.get(0)).optional()?;
    if let Some(remote) = remote {
        return Ok(remote);
    }
    if isolated_origin(tx, local)?.is_some() {
        return Err(StorageError::SharedCrossScopeMove);
    }
    // This is called only from an explicit local outbox route, never inbound.
    // Reusing a former wire ID after a binding fork would hijack the shared copy.
    let remote = if binding(tx, share, local)?.is_some() {
        ClipId::new().to_string()
    } else {
        local.to_owned()
    };
    tx.execute("INSERT INTO shared_clip_bindings (share_id, remote_clip_id, local_clip_id, private_origin) VALUES (?1, ?2, ?3, 1)", params![share, &remote, local])?;
    Ok(remote)
}

fn map_envelope(envelope: &SyncEnvelope, clip: &str) -> Result<SyncEnvelope, StorageError> {
    let mut result = envelope.clone();
    let clip_id = ClipId::from_str(clip).map_err(|error| corrupt("shared binding", error))?;
    match result.change.entity.kind {
        SyncEntityKind::Clip => {
            result.change.entity.id = clip.to_owned();
            if let Some(SyncPayload::Clip(item)) = &mut result.payload {
                item.id = clip_id;
            }
        }
        SyncEntityKind::PinboardMembership => {
            let (board, _) = result
                .change
                .entity
                .id
                .split_once(':')
                .ok_or(StorageError::InvalidRemoteSyncBatch)?;
            result.change.entity.id = format!("{board}:{clip}");
            if let Some(SyncPayload::PinboardMembership(item)) = &mut result.payload {
                item.clip_id = clip_id;
            }
        }
        SyncEntityKind::Pinboard => {}
    }
    result
        .validate()
        .map_err(|error| corrupt("mapped shared envelope", error))?;
    Ok(result)
}

fn referenced_clip(envelope: &SyncEnvelope) -> Option<&str> {
    match envelope.change.entity.kind {
        SyncEntityKind::Clip => Some(&envelope.change.entity.id),
        SyncEntityKind::PinboardMembership => envelope
            .change
            .entity
            .id
            .split_once(':')
            .map(|(_, clip)| clip),
        SyncEntityKind::Pinboard => None,
    }
}

/// Freeze every newly routed operation, including a clip snapshot reused by a
/// membership route. Old rows retain the exact old UUID/payload/protocol version.
pub(super) fn freeze_pending_wire(tx: &Transaction<'_>) -> Result<(), StorageError> {
    let rows = {
        let mut stmt = tx.prepare("SELECT so.share_id, s.pinboard_id, o.change_json FROM cloudkit_share_outbox so
            JOIN cloudkit_shares s ON s.id = so.share_id JOIN sync_outbox o ON o.operation_id = so.operation_id
            LEFT JOIN shared_wire_outbox w ON w.share_id = so.share_id AND w.operation_id = so.operation_id
            WHERE so.state = 'pending' AND w.operation_id IS NULL ORDER BY so.created_at_ms, o.logical_counter, o.operation_id")?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (share, board, json) in rows {
        let change: SyncChange =
            serde_json::from_str(&json).map_err(|error| corrupt("shared outbox", error))?;
        let mut blobs = BTreeMap::new();
        let (schema_version, payload) = load_sync_snapshot(tx, &change, &mut blobs)?;
        let mut envelope = SyncEnvelope {
            schema_version,
            scope: SyncScope::Shared,
            change,
            payload,
        };
        if let Some(local) = referenced_clip(&envelope) {
            let remote = bind_local(tx, &share, local)?;
            envelope = map_envelope(&envelope, &remote)?;
        }
        validate_share_envelope(
            &envelope,
            PinboardId::from_str(&board).map_err(|error| corrupt("shared board", error))?,
        )?;
        let json = encode(&envelope)?;
        tx.execute("INSERT INTO shared_wire_outbox (share_id, operation_id, envelope_json) VALUES (?1, ?2, ?3)",
            params![&share, envelope.change.operation_id.to_string(), &json])?;
        retain_envelope(tx, &share, &envelope, &blobs)?;
        merge_state(tx, &share, &envelope)?;
    }
    Ok(())
}

fn retain_envelope(
    tx: &Transaction<'_>,
    share: &str,
    envelope: &SyncEnvelope,
    blobs: &BTreeMap<[u8; 32], Vec<u8>>,
) -> Result<(), StorageError> {
    check_receipt(tx, share, envelope)?;
    let operation = envelope.change.operation_id.to_string();
    let existing: Option<String> = tx.query_row("SELECT envelope_json FROM cloudkit_share_inbox WHERE share_id = ?1 AND operation_id = ?2", params![share, &operation], |row| row.get(0)).optional()?;
    if let Some(existing) = existing {
        if decode(&existing)? != *envelope {
            return Err(StorageError::SharedOperationMismatch);
        }
        return Ok(());
    }
    let borrowed = blobs
        .iter()
        .map(|(hash, bytes)| (*hash, bytes.as_slice()))
        .collect();
    let attachments = validate_attachments(tx, share, envelope, &borrowed)?;
    tx.execute("INSERT INTO cloudkit_share_inbox (share_id, operation_id, envelope_json, received_at_ms) VALUES (?1, ?2, ?3, ?4)",
        params![share, &operation, encode(envelope)?, Utc::now().timestamp_millis()])?;
    for (hash, bytes) in attachments {
        tx.execute("INSERT OR IGNORE INTO cloudkit_share_inbox_blobs (share_id, content_hash, byte_len, bytes) VALUES (?1, ?2, ?3, ?4)",
            params![share, hash.as_slice(), to_i64(bytes.len())?, bytes])?;
        tx.execute("INSERT OR IGNORE INTO cloudkit_share_inbox_blob_refs (share_id, operation_id, content_hash) VALUES (?1, ?2, ?3)", params![share, &operation, hash.as_slice()])?;
    }
    Ok(())
}

pub(super) fn record_local_change(
    tx: &Transaction<'_>,
    change: &SyncChange,
) -> Result<(), StorageError> {
    if private_forbidden(tx, &change.entity)? {
        tx.execute(
            "INSERT OR IGNORE INTO shared_only_outbox (operation_id) VALUES (?1)",
            params![change.operation_id.to_string()],
        )?;
    }
    freeze_pending_wire(tx)?;
    if change.entity.kind == SyncEntityKind::PinboardMembership
        && change.change == SyncChangeKind::Delete
        && let Some((_, local)) = change.entity.id.split_once(':')
        && isolated_origin(tx, local)?.is_some()
    {
        hide_isolated(tx, local)?;
    }
    compact(tx)?;
    Ok(())
}

pub(super) fn private_forbidden(
    connection: &Connection,
    entity: &SyncEntity,
) -> Result<bool, StorageError> {
    match entity.kind {
        SyncEntityKind::Clip => Ok(isolated_origin(connection, &entity.id)?.is_some()),
        SyncEntityKind::Pinboard | SyncEntityKind::PinboardMembership => {
            if let Some((_, clip)) = entity.id.split_once(':')
                && isolated_origin(connection, clip)?.is_some()
            {
                return Ok(true);
            }
            let board = entity
                .id
                .split(':')
                .next()
                .ok_or(StorageError::InvalidRemoteSyncBatch)?;
            let participant: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM cloudkit_shares WHERE pinboard_id = ?1 AND role = 'participant')", params![board], |row| row.get(0))?;
            Ok(participant)
        }
    }
}

fn hide_isolated(tx: &Transaction<'_>, local: &str) -> Result<(), StorageError> {
    apply_remote_delete(
        tx,
        &SyncEntity {
            kind: SyncEntityKind::Clip,
            id: local.into(),
        },
    )?;
    tx.execute(
        "DELETE FROM preview_cache WHERE clip_id = ?1",
        params![local],
    )?;
    Ok(())
}

fn compact(tx: &Transaction<'_>) -> Result<(), StorageError> {
    tx.execute("DELETE FROM cloudkit_share_inbox AS i WHERE EXISTS(SELECT 1 FROM shared_operation_receipts r WHERE r.share_id = i.share_id AND r.operation_id = i.operation_id)
        AND NOT EXISTS(SELECT 1 FROM shared_entity_states s WHERE s.share_id = i.share_id AND s.operation_id = i.operation_id)
        AND NOT EXISTS(SELECT 1 FROM shared_conflicts c WHERE c.share_id = i.share_id AND (c.local_operation_id = i.operation_id OR c.remote_operation_id = i.operation_id))", [])?;
    tx.execute("DELETE FROM cloudkit_share_inbox_blobs AS b WHERE NOT EXISTS(SELECT 1 FROM cloudkit_share_inbox_blob_refs r WHERE r.share_id = b.share_id AND r.content_hash = b.content_hash)", [])?;
    Ok(())
}

impl SqliteStore {
    /// Applies at most `limit` operations and projections atomically. Canonical
    /// states keep out-of-order dependencies, including tombstones, across restarts.
    pub fn materialize_pinboard_share(
        &self,
        share_id: uuid::Uuid,
        limit: u32,
    ) -> Result<SharedMaterializationReport, StorageError> {
        if !(1..=250).contains(&limit) {
            return Err(StorageError::InvalidSyncBatchLimit);
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        let share = share_id.to_string();
        let board: String = tx
            .query_row(
                "SELECT pinboard_id FROM cloudkit_shares WHERE id = ?1 AND state = 'active'",
                params![&share],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::NotFound)?;
        let board_id =
            PinboardId::from_str(&board).map_err(|error| corrupt("shared board", error))?;
        let rows = {
            let mut stmt = tx.prepare("SELECT envelope_json FROM cloudkit_share_inbox i WHERE share_id = ?1
                AND NOT EXISTS(SELECT 1 FROM shared_operation_receipts r WHERE r.share_id = i.share_id AND r.operation_id = i.operation_id)
                ORDER BY received_at_ms, operation_id LIMIT ?2")?;
            stmt.query_map(params![&share, limit], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut report = SharedMaterializationReport::default();
        for json in rows {
            let envelope = decode(&json)?;
            validate_share_envelope(&envelope, board_id)?;
            validate_attachments(&tx, &share, &envelope, &BTreeMap::new())?;
            if merge_state(&tx, &share, &envelope)? {
                observe_remote_sync_clock(&tx, &envelope.change.timestamp)?;
                if let Some(remote) = referenced_clip(&envelope) {
                    tx.execute("INSERT OR IGNORE INTO shared_projection_queue (share_id, remote_clip_id) VALUES (?1, ?2)", params![&share, remote])?;
                } else {
                    // A board save changes presentation, never the local tab order.
                    if let Some(SyncPayload::Pinboard(item)) = &envelope.payload {
                        tx.execute("UPDATE pinboards SET name = ?2, color = ?3, updated_at_ms = ?4, is_shared = 1 WHERE id = ?1",
                            params![&board, item.name.trim(), normalize_pinboard_color(&item.color)?, item.updated_at.timestamp_millis()])?;
                        publish_local_projection(&tx, &envelope.change)?;
                    }
                    tx.execute("INSERT OR IGNORE INTO shared_projection_queue (share_id, remote_clip_id)
                        SELECT share_id, entity_id FROM shared_entity_states WHERE share_id = ?1 AND entity_kind = 'clip'", params![&share])?;
                }
            }
            report.consumed += 1;
        }
        let projected = {
            let mut stmt = tx.prepare("SELECT remote_clip_id FROM shared_projection_queue WHERE share_id = ?1 ORDER BY remote_clip_id LIMIT ?2")?;
            stmt.query_map(params![&share, limit], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for remote in projected {
            if project_clip(&tx, &share, &board, &remote)? {
                report.projected += 1;
            }
            tx.execute(
                "DELETE FROM shared_projection_queue WHERE share_id = ?1 AND remote_clip_id = ?2",
                params![&share, remote],
            )?;
        }
        report.pending = tx.query_row(
            "SELECT COUNT(*) FROM shared_projection_queue WHERE share_id = ?1",
            params![&share],
            |row| row.get::<_, u32>(0),
        )? as usize;
        report.conflicts = tx.query_row(
            "SELECT COUNT(*) FROM shared_conflicts WHERE share_id = ?1",
            params![&share],
            |row| row.get::<_, u32>(0),
        )? as usize;
        compact(&tx)?;
        delete_unreferenced_blobs(&tx)?;
        tx.commit()?;
        Ok(report)
    }

    pub fn shared_conflict_count(&self) -> Result<usize, StorageError> {
        Ok(self
            .lock()?
            .query_row("SELECT COUNT(*) FROM shared_conflicts", [], |row| {
                row.get::<_, u32>(0)
            })? as usize)
    }
}

fn project_clip(
    tx: &Transaction<'_>,
    share: &str,
    board: &str,
    remote: &str,
) -> Result<bool, StorageError> {
    let clip = state(tx, share, SyncEntityKind::Clip, remote)?;
    let member = state(
        tx,
        share,
        SyncEntityKind::PinboardMembership,
        &format!("{board}:{remote}"),
    )?;
    let board_state = state(tx, share, SyncEntityKind::Pinboard, board)?;
    let board_alive = board_state
        .as_ref()
        .is_none_or(|e| e.change.change == SyncChangeKind::Save);
    let member_alive = board_alive
        && member
            .as_ref()
            .is_some_and(|e| e.change.change == SyncChangeKind::Save);
    let known_binding = binding(tx, share, remote)?;
    if !member_alive {
        if let Some((local, private)) = known_binding {
            let removed = tx.execute(
                "DELETE FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2",
                params![board, &local],
            )?;
            if removed > 0
                && let Some(member) = &member
            {
                let mapped = map_envelope(member, &local)?;
                publish_local_projection(tx, &mapped.change)?;
            }
            if !private {
                hide_isolated(tx, &local)?;
            }
        }
        return Ok(false);
    }
    let Some(clip) = clip else {
        return Ok(false);
    };
    let (mut local, mut private) =
        known_binding.unwrap_or_else(|| (ClipId::new().to_string(), false));
    let current_member: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2)",
        params![board, &local],
        |row| row.get(0),
    )?;
    if private && !current_member {
        // The owner moved this original out. A later re-add is a shared copy,
        // not permission to mutate the now-private original at the old ID.
        local = ClipId::new().to_string();
        private = false;
    }
    if clip.change.change == SyncChangeKind::Delete {
        if current_member || !private {
            hide_isolated(tx, &local)?;
            if current_member {
                publish_local_projection(tx, &map_envelope(&clip, &local)?.change)?;
            }
        }
        return Ok(false);
    }
    tx.execute("INSERT INTO shared_clip_bindings (share_id, remote_clip_id, local_clip_id, private_origin) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(share_id, remote_clip_id) DO UPDATE SET local_clip_id = excluded.local_clip_id, private_origin = excluded.private_origin",
        params![share, remote, &local, private])?;
    // Every representation comes from this share. Never fall back to private blobs.
    let blobs = validate_attachments(tx, share, &clip, &BTreeMap::new())?;
    let borrowed = blobs
        .iter()
        .map(|(hash, bytes)| (*hash, bytes.as_slice()))
        .collect();
    let mapped = map_envelope(&clip, &local)?;
    let already_current =
        load_sync_entity_change(tx, &mapped.change.entity)?.is_some_and(|change| {
            change.version == mapped.change.version && change.change == mapped.change.change
        });
    if !already_current || !current_member {
        apply_remote_envelope(tx, &mapped, &borrowed)?;
        publish_local_projection(tx, &mapped.change)?;
    }
    if let Some(member) = member {
        let member = map_envelope(&member, &local)?;
        apply_remote_envelope(tx, &member, &BTreeMap::new())?;
        publish_local_projection(tx, &member.change)?;
    }
    Ok(true)
}

/// Shared changes to an explicitly shared private original must reach the
/// owner's other private devices. Use a fresh operation UUID and the same
/// causal version, without re-routing it to this or any other shared zone.
fn publish_local_projection(tx: &Transaction<'_>, change: &SyncChange) -> Result<(), StorageError> {
    if private_forbidden(tx, &change.entity)? {
        return store_remote_sync_change(tx, change);
    }
    if load_sync_entity_change(tx, &change.entity)?
        .is_some_and(|current| current.version == change.version && current.change == change.change)
    {
        return Ok(());
    }
    let mut mirror = change.clone();
    mirror.operation_id = uuid::Uuid::new_v4();
    let json =
        serde_json::to_string(&mirror).map_err(|error| corrupt("private projection", error))?;
    tx.execute("INSERT INTO sync_outbox (operation_id, entity_kind, entity_id, change_kind, change_json, state, created_at_ms, logical_counter, node_id, envelope_schema_version)
        VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9)",
        params![mirror.operation_id.to_string(), sync_entity_kind_key(mirror.entity.kind), &mirror.entity.id, sync_change_kind_key(mirror.change), json,
            mirror.timestamp.wall_time_ms, i64::from(mirror.timestamp.counter), mirror.timestamp.node_id.to_string(), paste_sync::SYNC_ENVELOPE_SCHEMA_VERSION])?;
    snapshot_sync_change(tx, &mirror)?;
    store_remote_sync_change(tx, &mirror)
}
