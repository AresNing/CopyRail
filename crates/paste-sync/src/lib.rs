#![forbid(unsafe_code)]

use std::{cmp::Ordering, collections::BTreeMap};

use chrono::{DateTime, Utc};
use paste_domain::{ClipId, ClipItem, DeviceId, Pinboard, PinboardId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const SYNC_ENVELOPE_SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HybridTimestamp {
    pub wall_time_ms: i64,
    pub counter: u32,
    pub node_id: DeviceId,
}

impl Ord for HybridTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.wall_time_ms
            .cmp(&other.wall_time_ms)
            .then_with(|| self.counter.cmp(&other.counter))
            .then_with(|| self.node_id.to_string().cmp(&other.node_id.to_string()))
    }
}

impl PartialOrd for HybridTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
pub struct HybridClock {
    node_id: DeviceId,
    last_wall_time_ms: i64,
    last_counter: u32,
}

impl HybridClock {
    pub fn new(node_id: DeviceId) -> Self {
        Self {
            node_id,
            last_wall_time_ms: i64::MIN,
            last_counter: 0,
        }
    }

    pub fn from_timestamp(timestamp: HybridTimestamp) -> Self {
        Self {
            node_id: timestamp.node_id,
            last_wall_time_ms: timestamp.wall_time_ms,
            last_counter: timestamp.counter,
        }
    }

    pub fn tick(&mut self, physical_time_ms: i64) -> HybridTimestamp {
        if physical_time_ms > self.last_wall_time_ms {
            self.last_wall_time_ms = physical_time_ms;
            self.last_counter = 0;
        } else {
            self.last_counter = self.last_counter.saturating_add(1);
        }
        self.timestamp()
    }

    pub fn observe(&mut self, remote: &HybridTimestamp, physical_time_ms: i64) -> HybridTimestamp {
        let wall_time_ms = physical_time_ms
            .max(self.last_wall_time_ms)
            .max(remote.wall_time_ms);
        let counter = match (
            wall_time_ms == self.last_wall_time_ms,
            wall_time_ms == remote.wall_time_ms,
        ) {
            (true, true) => self.last_counter.max(remote.counter).saturating_add(1),
            (true, false) => self.last_counter.saturating_add(1),
            (false, true) => remote.counter.saturating_add(1),
            (false, false) => 0,
        };
        self.last_wall_time_ms = wall_time_ms;
        self.last_counter = counter;
        self.timestamp()
    }

    pub fn timestamp(&self) -> HybridTimestamp {
        HybridTimestamp {
            wall_time_ms: self.last_wall_time_ms,
            counter: self.last_counter,
            node_id: self.node_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionOrder {
    Before,
    Equal,
    After,
    Concurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteDisposition {
    Apply,
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteMergeDecision {
    pub disposition: RemoteDisposition,
    pub preserve_losing_clip: bool,
}

pub fn decide_remote_change(
    local: Option<&SyncChange>,
    remote: &SyncChange,
) -> RemoteMergeDecision {
    let Some(local) = local else {
        return RemoteMergeDecision {
            disposition: RemoteDisposition::Apply,
            preserve_losing_clip: false,
        };
    };
    match local.version.compare(&remote.version) {
        VersionOrder::Before => RemoteMergeDecision {
            disposition: RemoteDisposition::Apply,
            preserve_losing_clip: false,
        },
        VersionOrder::After | VersionOrder::Equal => RemoteMergeDecision {
            disposition: RemoteDisposition::Ignore,
            preserve_losing_clip: false,
        },
        VersionOrder::Concurrent => {
            let remote_wins = local.timestamp.cmp(&remote.timestamp).then_with(|| {
                local
                    .operation_id
                    .as_bytes()
                    .cmp(remote.operation_id.as_bytes())
            }) == Ordering::Less;
            RemoteMergeDecision {
                disposition: if remote_wins {
                    RemoteDisposition::Apply
                } else {
                    RemoteDisposition::Ignore
                },
                preserve_losing_clip: local.entity.kind == SyncEntityKind::Clip
                    && (local.change == SyncChangeKind::Save
                        || remote.change == SyncChangeKind::Save),
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionVector(BTreeMap<String, u64>);

impl VersionVector {
    pub fn increment(&mut self, device_id: DeviceId) -> u64 {
        let value = self.0.entry(device_id.to_string()).or_default();
        *value = value.saturating_add(1);
        *value
    }

    pub fn merge(&mut self, other: &Self) {
        for (node, value) in &other.0 {
            let local = self.0.entry(node.clone()).or_default();
            *local = (*local).max(*value);
        }
    }

    pub fn compare(&self, other: &Self) -> VersionOrder {
        let mut lower = false;
        let mut higher = false;
        for node in self.0.keys().chain(other.0.keys()) {
            let left = self.0.get(node).copied().unwrap_or_default();
            let right = other.0.get(node).copied().unwrap_or_default();
            lower |= left < right;
            higher |= left > right;
        }
        match (lower, higher) {
            (false, false) => VersionOrder::Equal,
            (true, false) => VersionOrder::Before,
            (false, true) => VersionOrder::After,
            (true, true) => VersionOrder::Concurrent,
        }
    }

    pub fn entries(&self) -> &BTreeMap<String, u64> {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEntityKind {
    Clip,
    Pinboard,
    PinboardMembership,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncEntity {
    pub kind: SyncEntityKind,
    pub id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncChangeKind {
    Save,
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncChange {
    pub operation_id: Uuid,
    pub entity: SyncEntity,
    pub change: SyncChangeKind,
    pub timestamp: HybridTimestamp,
    pub version: VersionVector,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncScope {
    Private,
    Shared,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PinboardMembershipSnapshot {
    pub pinboard_id: PinboardId,
    pub clip_id: ClipId,
    pub position: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SyncPayload {
    Clip(ClipItem),
    Pinboard(Pinboard),
    PinboardMembership(PinboardMembershipSnapshot),
}

impl SyncPayload {
    fn entity(&self) -> SyncEntity {
        match self {
            Self::Clip(item) => SyncEntity {
                kind: SyncEntityKind::Clip,
                id: item.id.to_string(),
            },
            Self::Pinboard(item) => SyncEntity {
                kind: SyncEntityKind::Pinboard,
                id: item.id.to_string(),
            },
            Self::PinboardMembership(item) => SyncEntity {
                kind: SyncEntityKind::PinboardMembership,
                id: format!("{}:{}", item.pinboard_id, item.clip_id),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncEnvelope {
    pub schema_version: u16,
    pub scope: SyncScope,
    pub change: SyncChange,
    pub payload: Option<SyncPayload>,
}

impl SyncEnvelope {
    pub fn new(
        scope: SyncScope,
        change: SyncChange,
        payload: Option<SyncPayload>,
    ) -> Result<Self, SyncEnvelopeError> {
        let envelope = Self {
            schema_version: SYNC_ENVELOPE_SCHEMA_VERSION,
            scope,
            change,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), SyncEnvelopeError> {
        if !(1..=SYNC_ENVELOPE_SCHEMA_VERSION).contains(&self.schema_version) {
            return Err(SyncEnvelopeError::UnsupportedSchema(self.schema_version));
        }
        if let Some(SyncPayload::Clip(clip)) = &self.payload {
            for representation in &clip.representations {
                if self.schema_version == 1 && representation.native_type.is_some() {
                    return Err(SyncEnvelopeError::NativeTypeRequiresV2);
                }
                if representation
                    .native_type
                    .as_deref()
                    .is_some_and(|value| !paste_domain::valid_native_type(value))
                {
                    return Err(SyncEnvelopeError::InvalidNativeType);
                }
            }
        }
        match (self.change.change, self.payload.as_ref()) {
            (SyncChangeKind::Save, None) => return Err(SyncEnvelopeError::MissingSavePayload),
            (SyncChangeKind::Delete, Some(_)) => {
                return Err(SyncEnvelopeError::DeleteWithPayload);
            }
            (SyncChangeKind::Save, Some(payload)) if payload.entity() != self.change.entity => {
                return Err(SyncEnvelopeError::EntityMismatch);
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoalescedSyncChange {
    pub change: SyncChange,
    pub operation_ids: Vec<Uuid>,
}

pub fn coalesce_changes(changes: &[SyncChange]) -> Vec<CoalescedSyncChange> {
    let mut indexes = BTreeMap::<(String, String), usize>::new();
    let mut result = Vec::<CoalescedSyncChange>::new();
    for change in changes {
        let key = (
            match change.entity.kind {
                SyncEntityKind::Clip => "clip",
                SyncEntityKind::Pinboard => "pinboard",
                SyncEntityKind::PinboardMembership => "pinboard_membership",
            }
            .to_owned(),
            change.entity.id.clone(),
        );
        if let Some(index) = indexes.get(&key).copied() {
            let group = &mut result[index];
            group.change = change.clone();
            group.operation_ids.push(change.operation_id);
        } else {
            indexes.insert(key, result.len());
            result.push(CoalescedSyncChange {
                change: change.clone(),
                operation_ids: vec![change.operation_id],
            });
        }
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncBlob {
    pub content_hash: [u8; 32],
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSyncOperation {
    pub envelope: SyncEnvelope,
    pub acknowledge_operation_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreparedSyncBatch {
    pub operations: Vec<PreparedSyncOperation>,
    pub blobs: Vec<SyncBlob>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteConflict {
    pub local: SyncChange,
    pub remote: SyncEnvelope,
    pub decision: RemoteMergeDecision,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SyncEnvelopeError {
    #[error("native representation types require sync envelope version 2")]
    NativeTypeRequiresV2,
    #[error("native pasteboard type is invalid")]
    InvalidNativeType,
    #[error("unsupported sync envelope schema version {0}")]
    UnsupportedSchema(u16),
    #[error("save operations require an entity payload")]
    MissingSavePayload,
    #[error("delete operations must be represented as tombstones without a payload")]
    DeleteWithPayload,
    #[error("sync payload does not match the changed entity")]
    EntityMismatch,
}

impl SyncChange {
    pub fn new(
        entity: SyncEntity,
        change: SyncChangeKind,
        timestamp: HybridTimestamp,
        version: VersionVector,
    ) -> Self {
        Self {
            operation_id: Uuid::new_v4(),
            entity,
            change,
            timestamp,
            version,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VersionedValue<T> {
    pub operation_id: Uuid,
    pub value: T,
    pub timestamp: HybridTimestamp,
    pub version: VersionVector,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeResult<T> {
    pub winner: VersionedValue<T>,
    pub conflict_copy: Option<VersionedValue<T>>,
}

pub fn merge_versioned<T: Clone>(
    local: &VersionedValue<T>,
    remote: &VersionedValue<T>,
    preserve_concurrent_loser: bool,
) -> MergeResult<T> {
    let version_order = local.version.compare(&remote.version);
    let winner_is_local = match version_order {
        VersionOrder::After => true,
        VersionOrder::Before => false,
        VersionOrder::Equal | VersionOrder::Concurrent => {
            local.timestamp.cmp(&remote.timestamp).then_with(|| {
                local
                    .operation_id
                    .as_bytes()
                    .cmp(remote.operation_id.as_bytes())
            }) != Ordering::Less
        }
    };
    let (winner, loser) = if winner_is_local {
        (local, remote)
    } else {
        (remote, local)
    };
    MergeResult {
        winner: winner.clone(),
        conflict_copy: (version_order == VersionOrder::Concurrent && preserve_concurrent_loser)
            .then(|| loser.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_clock_stays_monotonic_when_wall_clock_moves_backwards() {
        let mut clock = HybridClock::new(DeviceId::new());
        let first = clock.tick(1_000);
        let second = clock.tick(900);
        assert!(second > first);
        assert_eq!(second.wall_time_ms, 1_000);
        assert_eq!(second.counter, 1);
    }

    #[test]
    fn observing_a_remote_clock_advances_beyond_both_inputs() {
        let local_device = DeviceId::new();
        let remote = HybridTimestamp {
            wall_time_ms: 2_000,
            counter: 8,
            node_id: DeviceId::new(),
        };
        let mut clock = HybridClock::new(local_device);
        clock.tick(1_500);
        let observed = clock.observe(&remote, 1_800);
        assert!(observed > remote);
        assert_eq!(observed.wall_time_ms, 2_000);
        assert_eq!(observed.counter, 9);
        assert_eq!(observed.node_id, local_device);
    }

    #[test]
    fn version_vectors_distinguish_causality_from_concurrency() {
        let first_device = DeviceId::new();
        let second_device = DeviceId::new();
        let mut base = VersionVector::default();
        base.increment(first_device);
        let mut descendant = base.clone();
        descendant.increment(second_device);
        assert_eq!(base.compare(&descendant), VersionOrder::Before);

        let mut branch = base.clone();
        branch.increment(first_device);
        assert_eq!(branch.compare(&descendant), VersionOrder::Concurrent);
        branch.merge(&descendant);
        assert_eq!(branch.compare(&descendant), VersionOrder::After);
    }

    #[test]
    fn concurrent_content_edits_keep_a_deterministic_conflict_copy() {
        let first_device = DeviceId::new();
        let second_device = DeviceId::new();
        let mut first_version = VersionVector::default();
        first_version.increment(first_device);
        let mut second_version = VersionVector::default();
        second_version.increment(second_device);
        let first = VersionedValue {
            operation_id: Uuid::from_u128(1),
            value: "first edit",
            timestamp: HybridTimestamp {
                wall_time_ms: 10,
                counter: 0,
                node_id: first_device,
            },
            version: first_version,
        };
        let second = VersionedValue {
            operation_id: Uuid::from_u128(2),
            value: "second edit",
            timestamp: HybridTimestamp {
                wall_time_ms: 11,
                counter: 0,
                node_id: second_device,
            },
            version: second_version,
        };
        let merged = merge_versioned(&first, &second, true);
        assert_eq!(merged.winner.value, "second edit");
        assert_eq!(
            merged.conflict_copy.expect("conflict copy").value,
            "first edit"
        );
    }

    #[test]
    fn coalesces_unsent_entity_changes_without_losing_acknowledgements() {
        let device = DeviceId::new();
        let entity = SyncEntity {
            kind: SyncEntityKind::Clip,
            id: ClipId::new().to_string(),
        };
        let mut version = VersionVector::default();
        version.increment(device);
        let first = SyncChange::new(
            entity.clone(),
            SyncChangeKind::Save,
            HybridTimestamp {
                wall_time_ms: 10,
                counter: 0,
                node_id: device,
            },
            version.clone(),
        );
        version.increment(device);
        let second = SyncChange::new(
            entity,
            SyncChangeKind::Delete,
            HybridTimestamp {
                wall_time_ms: 11,
                counter: 0,
                node_id: device,
            },
            version,
        );
        let groups = coalesce_changes(&[first.clone(), second.clone()]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].change, second);
        assert_eq!(
            groups[0].operation_ids,
            vec![first.operation_id, groups[0].change.operation_id]
        );
        let tombstone = SyncEnvelope::new(SyncScope::Private, groups[0].change.clone(), None)
            .expect("delete tombstone");
        assert!(tombstone.payload.is_none());
    }

    #[test]
    fn remote_merge_uses_causality_then_deterministic_conflict_order() {
        let first_device = DeviceId::new();
        let second_device = DeviceId::new();
        let entity = SyncEntity {
            kind: SyncEntityKind::Clip,
            id: ClipId::new().to_string(),
        };
        let mut local_version = VersionVector::default();
        local_version.increment(first_device);
        let mut remote_version = VersionVector::default();
        remote_version.increment(second_device);
        let local = SyncChange {
            operation_id: Uuid::from_u128(1),
            entity: entity.clone(),
            change: SyncChangeKind::Save,
            timestamp: HybridTimestamp {
                wall_time_ms: 20,
                counter: 0,
                node_id: first_device,
            },
            version: local_version,
        };
        let remote = SyncChange {
            operation_id: Uuid::from_u128(2),
            entity,
            change: SyncChangeKind::Save,
            timestamp: HybridTimestamp {
                wall_time_ms: 21,
                counter: 0,
                node_id: second_device,
            },
            version: remote_version,
        };
        assert_eq!(
            decide_remote_change(Some(&local), &remote),
            RemoteMergeDecision {
                disposition: RemoteDisposition::Apply,
                preserve_losing_clip: true,
            }
        );
        assert_eq!(
            decide_remote_change(Some(&remote), &local),
            RemoteMergeDecision {
                disposition: RemoteDisposition::Ignore,
                preserve_losing_clip: true,
            }
        );
    }
}
