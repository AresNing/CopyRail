//! A narrow, safe boundary between PasteRS sync data and Apple's CloudKit API.
//!
//! Record planning is platform-independent and deterministic. The Objective-C
//! bridge is isolated in `native` and is never reached merely by constructing a
//! configuration or upload plan.

#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use paste_sync::{
    PreparedSyncBatch, SyncBlob, SyncChangeKind, SyncEntityKind, SyncEnvelope, SyncPayload,
    SyncScope,
};
use thiserror::Error;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod native;

const MAX_CLOUDKIT_RECORDS_PER_OPERATION: usize = 250;
const MAX_ENVELOPE_BYTES: usize = 768 * 1024;
const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
const MAX_BATCH_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudKitConfiguration {
    container_identifier: String,
}

impl CloudKitConfiguration {
    pub fn new(container_identifier: impl Into<String>) -> Result<Self, CloudKitError> {
        let container_identifier = container_identifier.into();
        let is_valid = container_identifier.starts_with("iCloud.")
            && container_identifier.len() <= 255
            && container_identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
        if !is_valid {
            return Err(CloudKitError::InvalidContainerIdentifier);
        }
        Ok(Self {
            container_identifier,
        })
    }

    #[must_use]
    pub fn container_identifier(&self) -> &str {
        &self.container_identifier
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudKitAccountStatus {
    CouldNotDetermine,
    Available,
    Restricted,
    NoAccount,
    TemporarilyUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudSharePermission {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudShareRole {
    Owner,
    Participant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudSharePlan {
    pinboard_id: uuid::Uuid,
    title: String,
    color: String,
    zone_name: String,
    permission: CloudSharePermission,
}

impl CloudSharePlan {
    pub fn new(
        pinboard_id: uuid::Uuid,
        title: impl Into<String>,
        color: impl Into<String>,
        permission: CloudSharePermission,
    ) -> Result<Self, CloudKitError> {
        let title = title.into().trim().to_owned();
        let color = color.into().to_ascii_lowercase();
        if title.is_empty() || title.chars().count() > 80 {
            return Err(CloudKitError::InvalidShareMetadata);
        }
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CloudKitError::InvalidShareMetadata);
        }
        Ok(Self {
            pinboard_id,
            title,
            color,
            zone_name: format!("PasteShare_{pinboard_id}"),
            permission,
        })
    }

    #[must_use]
    pub const fn pinboard_id(&self) -> uuid::Uuid {
        self.pinboard_id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn color(&self) -> &str {
        &self.color
    }

    #[must_use]
    pub fn zone_name(&self) -> &str {
        &self.zone_name
    }

    #[must_use]
    pub const fn permission(&self) -> CloudSharePermission {
        self.permission
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudShareDescriptor {
    pub pinboard_id: uuid::Uuid,
    pub title: String,
    pub color: String,
    pub zone_name: String,
    pub owner_name: String,
    pub share_record_name: String,
    pub share_url: String,
    pub permission: CloudSharePermission,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloudRecordValue {
    String(String),
    Integer(i64),
    Boolean(bool),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudAsset {
    pub field_name: String,
    pub content_hash: [u8; 32],
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudRecordPlan {
    pub zone_name: String,
    pub zone_owner_name: Option<String>,
    pub record_type: &'static str,
    pub record_name: String,
    pub fields: BTreeMap<&'static str, CloudRecordValue>,
    pub assets: Vec<CloudAsset>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CloudUploadPlan {
    pub records: Vec<CloudRecordPlan>,
    pub acknowledge_operation_ids: Vec<uuid::Uuid>,
}

impl CloudUploadPlan {
    pub fn from_prepared(batch: &PreparedSyncBatch) -> Result<Self, CloudKitError> {
        Self::from_prepared_in_zone(batch, None)
    }

    pub fn from_prepared_for_share(
        batch: &PreparedSyncBatch,
        zone_name: &str,
        owner_name: &str,
    ) -> Result<Self, CloudKitError> {
        validate_share_record_component(zone_name)?;
        validate_share_record_component(owner_name)?;
        if !zone_name.starts_with("PasteShare_") {
            return Err(CloudKitError::InvalidShareMetadata);
        }
        Self::from_prepared_in_zone(batch, Some((zone_name, owner_name)))
    }

    fn from_prepared_in_zone(
        batch: &PreparedSyncBatch,
        share_destination: Option<(&str, &str)>,
    ) -> Result<Self, CloudKitError> {
        let mut blobs = BTreeMap::<[u8; 32], &[u8]>::new();
        for blob in &batch.blobs {
            if blob.bytes.len() > MAX_ASSET_BYTES {
                return Err(CloudKitError::AssetTooLarge(blob.bytes.len()));
            }
            let actual_hash = *blake3::hash(&blob.bytes).as_bytes();
            if actual_hash != blob.content_hash {
                return Err(CloudKitError::BlobHashMismatch);
            }
            blobs.insert(blob.content_hash, &blob.bytes);
        }

        let mut records = Vec::new();
        let mut acknowledge_operation_ids = Vec::new();
        let mut total_bytes = 0usize;

        for operation in &batch.operations {
            operation
                .envelope
                .validate()
                .map_err(|error| CloudKitError::InvalidEnvelope(error.to_string()))?;
            match (share_destination, operation.envelope.scope) {
                (None, SyncScope::Private) | (Some(_), SyncScope::Shared) => {}
                (None, SyncScope::Shared) => {
                    return Err(CloudKitError::PrivatePlanContainsSharedEnvelope);
                }
                (Some(_), SyncScope::Private) => {
                    return Err(CloudKitError::SharedPlanContainsPrivateEnvelope);
                }
            }
            if operation.acknowledge_operation_ids.is_empty() {
                return Err(CloudKitError::MissingAcknowledgements);
            }
            let mut operation_blob_hashes = BTreeSet::new();
            if let Some(SyncPayload::Clip(clip)) = operation.envelope.payload.as_ref() {
                for representation in &clip.representations {
                    operation_blob_hashes.insert(representation.content_hash);
                }
            }
            let mut assets = Vec::with_capacity(operation_blob_hashes.len());
            for hash in operation_blob_hashes {
                let bytes = blobs
                    .get(&hash)
                    .copied()
                    .ok_or_else(|| CloudKitError::MissingBlob(hex_hash(&hash)))?;
                total_bytes = total_bytes
                    .checked_add(bytes.len())
                    .ok_or(CloudKitError::BatchTooLarge)?;
                let hash_hex = hex_hash(&hash);
                assets.push(CloudAsset {
                    field_name: format!("asset_{hash_hex}"),
                    content_hash: hash,
                    bytes: bytes.to_vec(),
                });
            }

            let envelope_json = serde_json::to_vec(&operation.envelope)
                .map_err(|error| CloudKitError::Serialization(error.to_string()))?;
            if envelope_json.len() > MAX_ENVELOPE_BYTES {
                return Err(CloudKitError::EnvelopeTooLarge(envelope_json.len()));
            }
            total_bytes = total_bytes
                .checked_add(envelope_json.len())
                .ok_or(CloudKitError::BatchTooLarge)?;

            let scope = operation.envelope.scope;
            let entity_kind = operation.envelope.change.entity.kind;
            let record_name = change_record_name(operation.envelope.change.operation_id);
            let mut fields = BTreeMap::new();
            fields.insert(
                "schemaVersion",
                CloudRecordValue::Integer(i64::from(operation.envelope.schema_version)),
            );
            fields.insert(
                "scope",
                CloudRecordValue::String(scope_name(scope).to_owned()),
            );
            fields.insert(
                "entityKind",
                CloudRecordValue::String(entity_kind_name(entity_kind).to_owned()),
            );
            fields.insert(
                "entityID",
                CloudRecordValue::String(operation.envelope.change.entity.id.clone()),
            );
            fields.insert(
                "operationID",
                CloudRecordValue::String(operation.envelope.change.operation_id.to_string()),
            );
            fields.insert(
                "updatedAtMilliseconds",
                CloudRecordValue::Integer(operation.envelope.change.timestamp.wall_time_ms),
            );
            fields.insert(
                "isTombstone",
                CloudRecordValue::Boolean(
                    operation.envelope.change.change == SyncChangeKind::Delete,
                ),
            );
            fields.insert("envelope", CloudRecordValue::Bytes(envelope_json));
            records.push(CloudRecordPlan {
                zone_name: share_destination
                    .map_or_else(|| zone_name(scope).to_owned(), |value| value.0.to_owned()),
                zone_owner_name: share_destination.map(|value| value.1.to_owned()),
                record_type: "PasteChange",
                record_name,
                fields,
                assets,
            });
            acknowledge_operation_ids.extend(&operation.acknowledge_operation_ids);
        }

        if total_bytes > MAX_BATCH_BYTES {
            return Err(CloudKitError::BatchTooLarge);
        }
        if records.len() > MAX_CLOUDKIT_RECORDS_PER_OPERATION {
            return Err(CloudKitError::TooManyRecords(records.len()));
        }
        acknowledge_operation_ids.sort_unstable();
        acknowledge_operation_ids.dedup();
        Ok(Self {
            records,
            acknowledge_operation_ids,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CloudKitClient {
    configuration: CloudKitConfiguration,
}

impl CloudKitClient {
    #[must_use]
    pub const fn new(configuration: CloudKitConfiguration) -> Self {
        Self { configuration }
    }

    #[must_use]
    pub fn configuration(&self) -> &CloudKitConfiguration {
        &self.configuration
    }

    /// Checks the current code signature for both the CloudKit service and the
    /// configured container. This is local-only and never contacts iCloud.
    #[must_use]
    pub fn has_required_entitlements(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::has_required_entitlements(&self.configuration)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            false
        }
    }

    /// Queries the configured container without starting sync or modifying data.
    ///
    /// The caller must enforce the product's explicit opt-in and entitlement
    /// checks before invoking this method.
    pub fn account_status(
        &self,
        timeout: Duration,
    ) -> Result<CloudKitAccountStatus, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::account_status(&self.configuration, timeout)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = timeout;
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    /// Atomically uploads an already validated private-database plan.
    ///
    /// A successful receipt is the only point at which the caller may remove
    /// the included IDs from its local outbox. This method does not mutate local
    /// storage and does not run unless explicitly called.
    pub fn upload_private(
        &self,
        plan: &CloudUploadPlan,
        timeout: Duration,
    ) -> Result<CloudUploadReceipt, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        if plan.records.is_empty() {
            return Ok(CloudUploadReceipt {
                saved_record_count: 0,
                acknowledge_operation_ids: plan.acknowledge_operation_ids.clone(),
            });
        }
        if plan
            .records
            .iter()
            .any(|record| record.zone_name != "PastePrivate")
        {
            return Err(CloudKitError::PrivateUploadContainsSharedRecords);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::upload_private(&self.configuration, plan, timeout)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = timeout;
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    /// Fetches one bounded page of private-zone changes.
    ///
    /// The opaque token must only be persisted after the returned envelopes
    /// and blobs have been committed to local storage.
    pub fn fetch_private_changes(
        &self,
        previous_server_change_token: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<CloudDownloadBatch, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        if previous_server_change_token.is_some_and(|token| token.is_empty()) {
            return Err(CloudKitError::InvalidServerChangeToken);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::fetch_private_changes(
                &self.configuration,
                previous_server_change_token,
                timeout,
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = (previous_server_change_token, timeout);
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    /// Creates a zone-wide public-link share for one Pinboard in its own
    /// private-database custom zone. No local state is mutated by this method.
    pub fn create_pinboard_share(
        &self,
        plan: &CloudSharePlan,
        timeout: Duration,
    ) -> Result<CloudShareDescriptor, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::create_pinboard_share(&self.configuration, plan, timeout)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = (plan, timeout);
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    /// Fetches and accepts one CloudKit invitation URL. The URL is validated
    /// before the native API is reached and the returned metadata remains
    /// untrusted until every required PasteRS field has been checked.
    pub fn accept_pinboard_share(
        &self,
        invitation_url: &str,
        timeout: Duration,
    ) -> Result<CloudShareDescriptor, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        validate_invitation_url(invitation_url)?;
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::accept_pinboard_share(&self.configuration, invitation_url, timeout)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = (invitation_url, timeout);
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    pub fn upload_pinboard_share(
        &self,
        plan: &CloudUploadPlan,
        role: CloudShareRole,
        timeout: Duration,
    ) -> Result<CloudUploadReceipt, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        if plan.records.is_empty() {
            return Ok(CloudUploadReceipt {
                saved_record_count: 0,
                acknowledge_operation_ids: plan.acknowledge_operation_ids.clone(),
            });
        }
        let first = &plan.records[0];
        if !first.zone_name.starts_with("PasteShare_")
            || first.zone_owner_name.is_none()
            || plan.records.iter().any(|record| {
                record.zone_name != first.zone_name
                    || record.zone_owner_name != first.zone_owner_name
            })
        {
            return Err(CloudKitError::InvalidShareMetadata);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::upload_pinboard_share(&self.configuration, plan, role, timeout)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = (role, timeout);
            Err(CloudKitError::UnsupportedPlatform)
        }
    }

    pub fn fetch_pinboard_share_changes(
        &self,
        zone_name: &str,
        owner_name: &str,
        role: CloudShareRole,
        previous_server_change_token: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<CloudDownloadBatch, CloudKitError> {
        if timeout.is_zero() {
            return Err(CloudKitError::InvalidTimeout);
        }
        validate_share_record_component(zone_name)?;
        validate_share_record_component(owner_name)?;
        if !zone_name.starts_with("PasteShare_") {
            return Err(CloudKitError::InvalidShareMetadata);
        }
        if previous_server_change_token.is_some_and(|token| token.is_empty()) {
            return Err(CloudKitError::InvalidServerChangeToken);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            native::fetch_pinboard_share_changes(
                &self.configuration,
                zone_name,
                owner_name,
                role,
                previous_server_change_token,
                timeout,
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            let _ = (
                zone_name,
                owner_name,
                role,
                previous_server_change_token,
                timeout,
            );
            Err(CloudKitError::UnsupportedPlatform)
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CloudUploadReceipt {
    pub saved_record_count: usize,
    pub acknowledge_operation_ids: Vec<uuid::Uuid>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CloudDownloadBatch {
    pub envelopes: Vec<SyncEnvelope>,
    pub blobs: Vec<SyncBlob>,
    pub server_change_token: Vec<u8>,
    pub more_coming: bool,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CloudKitError {
    #[error(
        "CloudKit container identifier must start with iCloud. and contain only ASCII letters, digits, dots, or hyphens"
    )]
    InvalidContainerIdentifier,
    #[error("CloudKit account status timeout must be greater than zero")]
    InvalidTimeout,
    #[error("CloudKit is only supported on Apple platforms")]
    UnsupportedPlatform,
    #[error("CloudKit account status request timed out")]
    AccountStatusTimeout,
    #[error("CloudKit account status request failed")]
    AccountStatusRequestFailed,
    #[error("CloudKit zone preparation timed out")]
    ZonePreparationTimeout,
    #[error("CloudKit zone preparation failed")]
    ZonePreparationFailed,
    #[error("CloudKit upload timed out")]
    UploadTimeout,
    #[error("CloudKit upload failed")]
    UploadFailed,
    #[error("CloudKit download timed out")]
    DownloadTimeout,
    #[error("CloudKit download failed")]
    DownloadFailed,
    #[error("CloudKit returned an invalid PasteRS record")]
    InvalidCloudRecord,
    #[error("CloudKit returned a record deletion outside the PasteRS tombstone protocol")]
    UnexpectedCloudRecordDeletion,
    #[error("CloudKit server change token is invalid")]
    InvalidServerChangeToken,
    #[error("CloudKit server change token expired")]
    ServerChangeTokenExpired,
    #[error("CloudKit server change token could not be archived")]
    ServerChangeTokenArchiveFailed,
    #[error("CloudKit download exceeds the configured safety budget")]
    DownloadTooLarge,
    #[error("private CloudKit upload plan contains shared-zone records")]
    PrivateUploadContainsSharedRecords,
    #[error("private CloudKit plan contains a shared sync envelope")]
    PrivatePlanContainsSharedEnvelope,
    #[error("shared CloudKit plan contains a private sync envelope")]
    SharedPlanContainsPrivateEnvelope,
    #[error("failed to create the protected temporary CloudKit asset directory")]
    AssetDirectoryCreationFailed,
    #[error("failed to stage a protected temporary CloudKit asset")]
    AssetStagingFailed,
    #[error("invalid sync envelope: {0}")]
    InvalidEnvelope(String),
    #[error("failed to serialize sync envelope: {0}")]
    Serialization(String),
    #[error("sync operation has no local outbox IDs to acknowledge")]
    MissingAcknowledgements,
    #[error("required sync blob {0} is missing")]
    MissingBlob(String),
    #[error("sync blob content hash does not match its bytes")]
    BlobHashMismatch,
    #[error("sync envelope is too large for one CloudKit record: {0} bytes")]
    EnvelopeTooLarge(usize),
    #[error("sync asset is too large for one CloudKit record: {0} bytes")]
    AssetTooLarge(usize),
    #[error("CloudKit batch exceeds the configured byte budget")]
    BatchTooLarge,
    #[error("CloudKit operation would contain too many records: {0}")]
    TooManyRecords(usize),
    #[error("CloudKit Pinboard share metadata is invalid")]
    InvalidShareMetadata,
    #[error("CloudKit share invitation URL must be a bounded HTTPS URL without credentials")]
    InvalidShareInvitationUrl,
    #[error("CloudKit share creation timed out")]
    ShareCreationTimeout,
    #[error("CloudKit share creation failed")]
    ShareCreationFailed,
    #[error("CloudKit share invitation fetch or acceptance timed out")]
    ShareAcceptanceTimeout,
    #[error("CloudKit share invitation fetch or acceptance failed")]
    ShareAcceptanceFailed,
}

fn validate_invitation_url(value: &str) -> Result<(), CloudKitError> {
    if value.len() > 2_048 {
        return Err(CloudKitError::InvalidShareInvitationUrl);
    }
    let url = url::Url::parse(value).map_err(|_| CloudKitError::InvalidShareInvitationUrl)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CloudKitError::InvalidShareInvitationUrl);
    }
    Ok(())
}

fn validate_share_record_component(value: &str) -> Result<(), CloudKitError> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CloudKitError::InvalidShareMetadata);
    }
    Ok(())
}

fn scope_name(scope: SyncScope) -> &'static str {
    match scope {
        SyncScope::Private => "private",
        SyncScope::Shared => "shared",
    }
}

fn zone_name(scope: SyncScope) -> &'static str {
    match scope {
        SyncScope::Private => "PastePrivate",
        SyncScope::Shared => "PasteShared",
    }
}

fn entity_kind_name(kind: SyncEntityKind) -> &'static str {
    match kind {
        SyncEntityKind::Clip => "clip",
        SyncEntityKind::Pinboard => "pinboard",
        SyncEntityKind::PinboardMembership => "pinboard_membership",
    }
}

fn change_record_name(operation_id: uuid::Uuid) -> String {
    format!("change_{operation_id}")
}

fn hex_hash(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use paste_domain::{
        ClipId, ClipItem, ContentKind, DeviceId, DeviceMetadata, PersistedRepresentation,
        RepresentationKind, SourceApplication,
    };
    use paste_sync::{
        HybridTimestamp, PreparedSyncOperation, SyncBlob, SyncChange, SyncEntity, SyncEnvelope,
        VersionVector,
    };
    use uuid::Uuid;

    use super::*;

    #[test]
    fn validates_container_identifiers_without_touching_cloudkit() {
        let configuration = CloudKitConfiguration::new("iCloud.io.pasters.desktop")
            .expect("test container identifier should be valid");
        assert_eq!(
            configuration.container_identifier(),
            "iCloud.io.pasters.desktop"
        );
        assert_eq!(
            CloudKitConfiguration::new("io.pasters.desktop"),
            Err(CloudKitError::InvalidContainerIdentifier)
        );
        assert_eq!(
            CloudKitConfiguration::new("iCloud.io.pasters desktop"),
            Err(CloudKitError::InvalidContainerIdentifier)
        );
    }

    #[test]
    fn plans_assets_before_changes_and_preserves_all_acknowledgements() {
        let bytes = b"hello CloudKit".to_vec();
        let content_hash = *blake3::hash(&bytes).as_bytes();
        let clip_id = ClipId::new();
        let change = SyncChange {
            operation_id: Uuid::new_v4(),
            entity: SyncEntity {
                kind: SyncEntityKind::Clip,
                id: clip_id.to_string(),
            },
            change: SyncChangeKind::Save,
            timestamp: HybridTimestamp {
                wall_time_ms: 42,
                counter: 0,
                node_id: DeviceId::new(),
            },
            version: VersionVector::default(),
        };
        let envelope = SyncEnvelope::new(
            SyncScope::Private,
            change,
            Some(SyncPayload::Clip(ClipItem {
                id: clip_id,
                captured_at: Utc
                    .timestamp_millis_opt(42)
                    .single()
                    .expect("test timestamp should be valid"),
                last_copied_at: Utc
                    .timestamp_millis_opt(42)
                    .single()
                    .expect("test timestamp should be valid"),
                source: SourceApplication::unknown(),
                device: DeviceMetadata {
                    id: DeviceId::new(),
                    display_name: "Test Mac".into(),
                },
                content_kind: ContentKind::Text,
                title: "hello".into(),
                searchable_text: "hello".into(),
                content_hash,
                representations: vec![PersistedRepresentation {
                    native_type: Some("public.utf8-plain-text".into()),
                    kind: RepresentationKind::PlainText,
                    mime_type: Some("text/plain".into()),
                    file_name: None,
                    byte_len: bytes.len() as u64,
                    content_hash,
                    text_preview: Some("hello".into()),
                }],
            })),
        )
        .expect("test envelope should be valid");
        let first_ack = Uuid::new_v4();
        let second_ack = Uuid::new_v4();
        let batch = PreparedSyncBatch {
            operations: vec![PreparedSyncOperation {
                envelope,
                acknowledge_operation_ids: vec![first_ack, second_ack, first_ack],
            }],
            blobs: vec![SyncBlob {
                content_hash,
                bytes,
            }],
        };

        let plan = CloudUploadPlan::from_prepared(&batch)
            .expect("valid prepared batch should produce a plan");
        assert_eq!(plan.records.len(), 1);
        assert_eq!(plan.records[0].record_type, "PasteChange");
        assert_eq!(plan.records[0].zone_name, "PastePrivate");
        assert_eq!(plan.records[0].assets.len(), 1);
        assert_eq!(plan.records[0].assets[0].content_hash, content_hash);
        assert_eq!(plan.acknowledge_operation_ids.len(), 2);
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let (round_tripped_envelope, round_tripped_blobs) =
                native::round_trip_record_plan(&plan.records[0], SyncScope::Private)
                    .expect("native CKRecord should round-trip locally");
            assert_eq!(round_tripped_envelope, batch.operations[0].envelope);
            assert_eq!(round_tripped_blobs, batch.blobs);
            assert!(matches!(
                native::round_trip_record_plan(&plan.records[0], SyncScope::Shared),
                Err(CloudKitError::InvalidCloudRecord)
            ));
            let mut mismatched = plan.records[0].clone();
            mismatched.record_name = change_record_name(Uuid::new_v4());
            assert!(matches!(
                native::round_trip_record_plan(&mismatched, SyncScope::Private),
                Err(CloudKitError::InvalidCloudRecord)
            ));
            let mut shared_batch = batch.clone();
            shared_batch.operations[0].envelope.scope = SyncScope::Shared;
            let shared_plan = CloudUploadPlan::from_prepared_for_share(
                &shared_batch,
                &format!("PasteShare_{}", Uuid::new_v4()),
                "test_owner",
            )
            .expect("shared plan");
            let (shared_envelope, shared_blobs) =
                native::round_trip_record_plan(&shared_plan.records[0], SyncScope::Shared)
                    .expect("shared CKRecord round trip without network");
            assert_eq!(shared_envelope, shared_batch.operations[0].envelope);
            assert_eq!(shared_blobs, shared_batch.blobs);
        }
    }

    #[test]
    fn refuses_to_plan_a_clip_when_an_asset_is_missing() {
        let bytes = b"missing".to_vec();
        let content_hash = *blake3::hash(&bytes).as_bytes();
        let clip_id = ClipId::new();
        let change = SyncChange {
            operation_id: Uuid::new_v4(),
            entity: SyncEntity {
                kind: SyncEntityKind::Clip,
                id: clip_id.to_string(),
            },
            change: SyncChangeKind::Save,
            timestamp: HybridTimestamp {
                wall_time_ms: 1,
                counter: 0,
                node_id: DeviceId::new(),
            },
            version: VersionVector::default(),
        };
        let envelope = SyncEnvelope::new(
            SyncScope::Private,
            change,
            Some(SyncPayload::Clip(ClipItem {
                id: clip_id,
                captured_at: Utc
                    .timestamp_millis_opt(1)
                    .single()
                    .expect("test timestamp should be valid"),
                last_copied_at: Utc
                    .timestamp_millis_opt(1)
                    .single()
                    .expect("test timestamp should be valid"),
                source: SourceApplication::unknown(),
                device: DeviceMetadata {
                    id: DeviceId::new(),
                    display_name: "Test Mac".into(),
                },
                content_kind: ContentKind::Text,
                title: "missing".into(),
                searchable_text: "missing".into(),
                content_hash,
                representations: vec![PersistedRepresentation {
                    native_type: None,
                    kind: RepresentationKind::PlainText,
                    mime_type: Some("text/plain".into()),
                    file_name: None,
                    byte_len: bytes.len() as u64,
                    content_hash,
                    text_preview: Some("missing".into()),
                }],
            })),
        )
        .expect("test envelope should be valid");
        let batch = PreparedSyncBatch {
            operations: vec![PreparedSyncOperation {
                envelope,
                acknowledge_operation_ids: vec![Uuid::new_v4()],
            }],
            blobs: vec![],
        };

        assert!(matches!(
            CloudUploadPlan::from_prepared(&batch),
            Err(CloudKitError::MissingBlob(_))
        ));
    }

    #[test]
    fn deletion_is_an_immutable_tombstone_record() {
        let operation_id = Uuid::new_v4();
        let change = SyncChange {
            operation_id,
            entity: SyncEntity {
                kind: SyncEntityKind::Pinboard,
                id: Uuid::new_v4().to_string(),
            },
            change: SyncChangeKind::Delete,
            timestamp: HybridTimestamp {
                wall_time_ms: 99,
                counter: 0,
                node_id: DeviceId::new(),
            },
            version: VersionVector::default(),
        };
        let envelope = SyncEnvelope::new(SyncScope::Private, change, None)
            .expect("delete tombstone envelope should be valid");
        let acknowledgement = Uuid::new_v4();
        let batch = PreparedSyncBatch {
            operations: vec![PreparedSyncOperation {
                envelope,
                acknowledge_operation_ids: vec![acknowledgement],
            }],
            blobs: vec![],
        };

        let first =
            CloudUploadPlan::from_prepared(&batch).expect("delete tombstone should produce a plan");
        let retry =
            CloudUploadPlan::from_prepared(&batch).expect("retry should produce the same plan");
        assert_eq!(
            first.records[0].record_name,
            format!("change_{operation_id}")
        );
        assert_eq!(first.records[0].record_name, retry.records[0].record_name);
        assert_eq!(
            first.records[0].fields.get("isTombstone"),
            Some(&CloudRecordValue::Boolean(true))
        );
    }

    #[test]
    fn empty_and_shared_private_upload_checks_do_not_touch_cloudkit() {
        let client = CloudKitClient::new(
            CloudKitConfiguration::new("iCloud.io.pasters.desktop")
                .expect("test container identifier should be valid"),
        );
        assert_eq!(
            client
                .upload_private(&CloudUploadPlan::default(), Duration::from_secs(1))
                .expect("empty plan should succeed without accessing CloudKit"),
            CloudUploadReceipt::default()
        );

        let plan = CloudUploadPlan {
            records: vec![CloudRecordPlan {
                zone_name: "PasteShared".into(),
                zone_owner_name: None,
                record_type: "PasteChange",
                record_name: "change_test".into(),
                fields: BTreeMap::new(),
                assets: vec![],
            }],
            acknowledge_operation_ids: vec![],
        };
        assert_eq!(
            client.upload_private(&plan, Duration::from_secs(1)),
            Err(CloudKitError::PrivateUploadContainsSharedRecords)
        );
        assert_eq!(
            client.fetch_private_changes(Some(&[]), Duration::from_secs(1)),
            Err(CloudKitError::InvalidServerChangeToken)
        );
        let _ = client.has_required_entitlements();
    }

    #[test]
    fn validates_pinboard_share_plans_and_invites_without_touching_cloudkit() {
        let pinboard_id = Uuid::new_v4();
        let plan = CloudSharePlan::new(
            pinboard_id,
            "Team snippets",
            "#FF9500",
            CloudSharePermission::ReadWrite,
        )
        .expect("valid share plan");
        assert_eq!(plan.pinboard_id(), pinboard_id);
        assert_eq!(plan.title(), "Team snippets");
        assert_eq!(plan.color(), "#ff9500");
        assert_eq!(plan.zone_name(), format!("PasteShare_{pinboard_id}"));
        assert_eq!(plan.permission(), CloudSharePermission::ReadWrite);
        assert_eq!(
            CloudSharePlan::new(pinboard_id, "", "#ff9500", CloudSharePermission::ReadOnly),
            Err(CloudKitError::InvalidShareMetadata)
        );
        assert_eq!(
            CloudSharePlan::new(
                pinboard_id,
                "Valid",
                "orange",
                CloudSharePermission::ReadOnly
            ),
            Err(CloudKitError::InvalidShareMetadata)
        );

        let client = CloudKitClient::new(
            CloudKitConfiguration::new("iCloud.io.pasters.desktop")
                .expect("test container identifier"),
        );
        assert_eq!(
            client.create_pinboard_share(&plan, Duration::ZERO),
            Err(CloudKitError::InvalidTimeout)
        );
        assert_eq!(
            client.accept_pinboard_share("http://example.com/share", Duration::from_secs(1)),
            Err(CloudKitError::InvalidShareInvitationUrl)
        );
        assert_eq!(
            client.accept_pinboard_share(
                "https://user:password@example.com/share",
                Duration::from_secs(1)
            ),
            Err(CloudKitError::InvalidShareInvitationUrl)
        );
    }
}
