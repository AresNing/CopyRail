use super::*;

/// Receiving a choice that causally includes both sides also clears that
/// conflict on peers. A normal edit based on only the winner cannot do this.
pub(super) fn clear_dominated_conflicts(
    tx: &Transaction<'_>,
    share: &str,
    change: &SyncChange,
) -> Result<(), StorageError> {
    if change.entity.kind != SyncEntityKind::Clip {
        return Ok(());
    }
    let mut statement = tx.prepare("SELECT c.local_operation_id, c.remote_operation_id,
        json_extract(a.envelope_json, '$.change.version'), json_extract(b.envelope_json, '$.change.version')
        FROM shared_conflicts c JOIN cloudkit_share_inbox a ON a.share_id = c.share_id AND a.operation_id = c.local_operation_id
        JOIN cloudkit_share_inbox b ON b.share_id = c.share_id AND b.operation_id = c.remote_operation_id
        WHERE c.share_id = ?1 AND json_extract(a.envelope_json, '$.change.entity.id') = ?2")?;
    let rows = statement.query_map(params![share, &change.entity.id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut remove = Vec::new();
    for row in rows {
        let (first, second, first_version, second_version) = row?;
        let first_version: VersionVector = serde_json::from_str(&first_version)
            .map_err(|error| corrupt("shared conflict version", error))?;
        let second_version: VersionVector = serde_json::from_str(&second_version)
            .map_err(|error| corrupt("shared conflict version", error))?;
        if change.version.compare(&first_version) == paste_sync::VersionOrder::After
            && change.version.compare(&second_version) == paste_sync::VersionOrder::After
        {
            remove.push((first, second));
        }
    }
    drop(statement);
    for (first, second) in remove {
        tx.execute("DELETE FROM shared_conflicts WHERE share_id = ?1 AND local_operation_id = ?2 AND remote_operation_id = ?3", params![share, first, second])?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedConflictId {
    pub share_id: uuid::Uuid,
    pub first_operation_id: uuid::Uuid,
    pub second_operation_id: uuid::Uuid,
}

impl std::fmt::Display for SharedConflictId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}",
            self.share_id, self.first_operation_id, self.second_operation_id
        )
    }
}

impl FromStr for SharedConflictId {
    type Err = StorageError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 110 {
            return Err(StorageError::InvalidSharedConflictId);
        }
        let parts = value.split('/').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err(StorageError::InvalidSharedConflictId);
        }
        let parse =
            |part| uuid::Uuid::parse_str(part).map_err(|_| StorageError::InvalidSharedConflictId);
        Ok(Self {
            share_id: parse(parts[0])?,
            first_operation_id: parse(parts[1])?,
            second_operation_id: parse(parts[2])?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedConflictResolution {
    KeepCurrent,
    UseFirst,
    UseSecond,
}

impl SharedConflictResolution {
    const fn key(self) -> &'static str {
        match self {
            Self::KeepCurrent => "keep_current",
            Self::UseFirst => "use_first",
            Self::UseSecond => "use_second",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedConflictVersion {
    pub title: String,
    pub preview: String,
    pub device_name: String,
    pub deleted: bool,
    pub timestamp_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedConflictSummary {
    pub id: SharedConflictId,
    pub pinboard_name: String,
    pub can_resolve: bool,
    pub current_operation_id: uuid::Uuid,
    pub current: SharedConflictVersion,
    pub first: SharedConflictVersion,
    pub second: SharedConflictVersion,
}

// Extract bounded display fields in SQLite instead of repeatedly loading three
// potentially 768 KiB envelopes per card into the desktop's polling response.
fn preview(
    connection: &Connection,
    share: &str,
    operation: &str,
) -> Result<SharedConflictVersion, StorageError> {
    Ok(connection.query_row("SELECT
        COALESCE(substr(json_extract(envelope_json, '$.payload.value.title'), 1, 80), '已删除'),
        COALESCE(substr(json_extract(envelope_json, '$.payload.value.searchable_text'), 1, 512), ''),
        COALESCE(substr(json_extract(envelope_json, '$.payload.value.device.display_name'), 1, 80), ''),
        json_extract(envelope_json, '$.change.change') = 'delete',
        json_extract(envelope_json, '$.change.timestamp.wall_time_ms')
        FROM cloudkit_share_inbox WHERE share_id = ?1 AND operation_id = ?2",
        params![share, operation], |row| Ok(SharedConflictVersion { title: row.get(0)?, preview: row.get(1)?, device_name: row.get(2)?, deleted: row.get(3)?, timestamp_ms: row.get(4)? }))?)
}

fn load_operation(
    connection: &Connection,
    share: &str,
    operation: uuid::Uuid,
) -> Result<SyncEnvelope, StorageError> {
    let json: String = connection.query_row(
        "SELECT envelope_json FROM cloudkit_share_inbox WHERE share_id = ?1 AND operation_id = ?2",
        params![share, operation.to_string()],
        |row| row.get(0),
    )?;
    decode(&json)
}

impl SqliteStore {
    pub fn list_shared_conflicts(
        &self,
        limit: u32,
    ) -> Result<Vec<SharedConflictSummary>, StorageError> {
        if !(1..=100).contains(&limit) {
            return Err(StorageError::InvalidSyncConflictLimit);
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT c.share_id, c.local_operation_id, c.remote_operation_id,
            p.name, s.role = 'owner' OR s.permission = 'read_write', current.operation_id
            FROM shared_conflicts c JOIN cloudkit_shares s ON s.id = c.share_id
            JOIN pinboards p ON p.id = s.pinboard_id
            JOIN cloudkit_share_inbox i ON i.share_id = c.share_id AND i.operation_id = c.local_operation_id
            JOIN shared_entity_states current ON current.share_id = c.share_id AND current.entity_kind = 'clip'
                AND current.entity_id = json_extract(i.envelope_json, '$.change.entity.id')
            WHERE s.state = 'active' ORDER BY c.share_id, c.local_operation_id, c.remote_operation_id LIMIT ?1")?;
        let rows = statement
            .query_map(params![limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = Vec::with_capacity(rows.len());
        for (share, first, second, board, writable, current) in rows {
            result.push(SharedConflictSummary {
                id: format!("{share}/{first}/{second}").parse()?,
                pinboard_name: board,
                can_resolve: writable,
                current_operation_id: uuid::Uuid::parse_str(&current)
                    .map_err(|_| StorageError::InvalidSharedConflictId)?,
                current: preview(&connection, &share, &current)?,
                first: preview(&connection, &share, &first)?,
                second: preview(&connection, &share, &second)?,
            });
        }
        Ok(result)
    }

    /// A choice is a new causal operation, not restoration of an old version
    /// number. Returns false for a decision already durably recorded. The CAS
    /// protects content changed after the displayed preview, and all mutations
    /// (including outbox supersession and conflict cleanup) share one transaction.
    pub fn resolve_shared_conflict(
        &self,
        id: SharedConflictId,
        expected_current: uuid::Uuid,
        resolution: SharedConflictResolution,
    ) -> Result<bool, StorageError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        let share = id.share_id.to_string();
        let first_id = id.first_operation_id.to_string();
        let second_id = id.second_operation_id.to_string();
        let done: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_conflict_decisions WHERE share_id = ?1 AND first_operation_id = ?2 AND second_operation_id = ?3)", params![&share, &first_id, &second_id], |row| row.get(0))?;
        if done {
            return Ok(false);
        }
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_conflicts WHERE share_id = ?1 AND local_operation_id = ?2 AND remote_operation_id = ?3)", params![&share, &first_id, &second_id], |row| row.get(0))?;
        if !exists {
            return Err(StorageError::NotFound);
        }
        let (board, writable): (String, bool) = tx.query_row("SELECT pinboard_id, role = 'owner' OR permission = 'read_write' FROM cloudkit_shares WHERE id = ?1 AND state = 'active'", params![&share], |row| Ok((row.get(0)?, row.get(1)?))).optional()?.ok_or(StorageError::NotFound)?;
        if !writable {
            return Err(StorageError::ReadOnlyPinboardShare);
        }
        let board_id = PinboardId::from_str(&board)
            .map_err(|error| corrupt("shared conflict board", error))?;
        let first = load_operation(&tx, &share, id.first_operation_id)?;
        let second = load_operation(&tx, &share, id.second_operation_id)?;
        validate_share_envelope(&first, board_id)?;
        validate_share_envelope(&second, board_id)?;
        if first.change.entity != second.change.entity
            || first.change.entity.kind != SyncEntityKind::Clip
        {
            return Err(StorageError::InvalidSharedConflictId);
        }
        let current = state(&tx, &share, SyncEntityKind::Clip, &first.change.entity.id)?
            .ok_or(StorageError::NotFound)?;
        if current.change.operation_id != expected_current {
            return Err(StorageError::StaleSharedConflict);
        }
        let remote = &current.change.entity.id;
        let selected = match resolution {
            SharedConflictResolution::KeepCurrent => &current,
            SharedConflictResolution::UseFirst => &first,
            SharedConflictResolution::UseSecond => &second,
        };
        let blobs = validate_attachments(&tx, &share, selected, &BTreeMap::new())?;
        let local = resolution_binding(&tx, &share, &board, remote, &current.change)?;
        let mut version = current.change.version.clone();
        version.merge(&first.change.version);
        version.merge(&second.change.version);
        let device = local_device_id(&tx)?;
        version.increment(device);
        for envelope in [&first, &second, &current] {
            observe_remote_sync_clock(&tx, &envelope.change.timestamp)?;
        }
        let (wall_time_ms, counter): (i64, u32) = tx.query_row(
            "SELECT wall_time_ms, counter FROM sync_clocks WHERE device_id = ?1",
            params![device.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let envelope = SyncEnvelope::new(
            SyncScope::Shared,
            SyncChange::new(
                current.change.entity.clone(),
                selected.change.change,
                HybridTimestamp {
                    wall_time_ms,
                    counter,
                    node_id: device,
                },
                version,
            ),
            selected.payload.clone(),
        )
        .map_err(|error| corrupt("shared conflict choice", error))?;
        enqueue_choice(&tx, &share, &local, &envelope, &blobs)?;
        retain_envelope(&tx, &share, &envelope, &blobs)?;
        merge_state(&tx, &share, &envelope)?;
        project_clip(&tx, &share, &board, remote)?;
        let operation = envelope.change.operation_id.to_string();
        // Only this share's older deliveries are superseded. Never acknowledge
        // private outbox rows or another zone's deliveries as a side effect.
        tx.execute(
            "UPDATE cloudkit_share_outbox SET state = 'acknowledged', acknowledged_at_ms = ?1
            WHERE share_id = ?2 AND operation_id != ?3 AND state = 'pending' AND operation_id IN (
                SELECT operation_id FROM shared_wire_outbox WHERE share_id = ?2
                AND json_extract(envelope_json, '$.change.entity.kind') = 'clip'
                AND json_extract(envelope_json, '$.change.entity.id') = ?4)",
            params![Utc::now().timestamp_millis(), &share, &operation, remote],
        )?;
        tx.execute("INSERT INTO shared_conflict_decisions (share_id, first_operation_id, second_operation_id, resolution, resolved_operation_id, resolved_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![&share, &first_id, &second_id, resolution.key(), &operation, Utc::now().timestamp_millis()])?;
        tx.execute("DELETE FROM shared_conflicts WHERE share_id = ?1 AND local_operation_id = ?2 AND remote_operation_id = ?3", params![&share, &first_id, &second_id])?;
        compact(&tx)?;
        delete_unreferenced_blobs(&tx)?;
        tx.commit()?;
        Ok(true)
    }
}

fn resolution_binding(
    tx: &Transaction<'_>,
    share: &str,
    board: &str,
    remote: &str,
    current: &SyncChange,
) -> Result<String, StorageError> {
    if let Some((local, private)) = binding(tx, share, remote)? {
        let member: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pinboard_items WHERE pinboard_id = ?1 AND clip_id = ?2)",
            params![board, &local],
            |row| row.get(0),
        )?;
        if private && member {
            ensure_clip_share_writable(tx, &local)?;
            // Private-only incoming updates have not necessarily reached the
            // share state yet. Do not overwrite that newer original blindly.
            if let Some(local_change) = load_sync_entity_change(
                tx,
                &SyncEntity {
                    kind: SyncEntityKind::Clip,
                    id: local.clone(),
                },
            )? && local_change.version != current.version
            {
                return Err(StorageError::StaleSharedConflict);
            }
        }
        return Ok(local);
    }
    let local = ClipId::new().to_string();
    tx.execute("INSERT INTO shared_clip_bindings (share_id, remote_clip_id, local_clip_id, private_origin) VALUES (?1, ?2, ?3, 0)", params![share, remote, &local])?;
    Ok(local)
}

fn enqueue_choice(
    tx: &Transaction<'_>,
    share: &str,
    local: &str,
    envelope: &SyncEnvelope,
    blobs: &BTreeMap<[u8; 32], Vec<u8>>,
) -> Result<(), StorageError> {
    let mapped = map_envelope(envelope, local)?;
    let operation = envelope.change.operation_id.to_string();
    let change = &mapped.change;
    tx.execute("INSERT INTO sync_outbox (operation_id, entity_kind, entity_id, change_kind, change_json, state, created_at_ms, logical_counter, node_id, envelope_schema_version)
        VALUES (?1, 'clip', ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8)", params![&operation, local, sync_change_kind_key(change.change), serde_json::to_string(change).map_err(|error| corrupt("shared choice", error))?,
            change.timestamp.wall_time_ms, i64::from(change.timestamp.counter), change.timestamp.node_id.to_string(), envelope.schema_version])?;
    tx.execute(
        "INSERT INTO shared_only_outbox (operation_id) VALUES (?1)",
        params![&operation],
    )?;
    tx.execute(
        "INSERT INTO sync_outbox_snapshots (operation_id, payload_json) VALUES (?1, ?2)",
        params![
            &operation,
            serde_json::to_string(&mapped.payload)
                .map_err(|error| corrupt("shared choice payload", error))?
        ],
    )?;
    for (hash, bytes) in blobs {
        tx.execute("INSERT OR IGNORE INTO blobs (content_hash, byte_len, bytes, created_at_ms) VALUES (?1, ?2, ?3, ?4)", params![hash.as_slice(), to_i64(bytes.len())?, bytes, Utc::now().timestamp_millis()])?;
        tx.execute(
            "INSERT INTO sync_outbox_blob_refs (operation_id, content_hash) VALUES (?1, ?2)",
            params![&operation, hash.as_slice()],
        )?;
    }
    tx.execute("INSERT INTO cloudkit_share_outbox (share_id, operation_id, state, created_at_ms) VALUES (?1, ?2, 'pending', ?3)", params![share, &operation, Utc::now().timestamp_millis()])?;
    tx.execute("INSERT INTO shared_wire_outbox (share_id, operation_id, envelope_json) VALUES (?1, ?2, ?3)", params![share, &operation, encode(envelope)?])?;
    Ok(())
}
