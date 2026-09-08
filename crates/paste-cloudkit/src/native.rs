//! The only Objective-C/CloudKit FFI boundary in this crate.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use block2::RcBlock;
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    string::CFString,
};
use core_foundation_sys::{
    base::{CFAllocatorRef, CFRelease, CFTypeRef},
    error::CFErrorRef,
    string::CFStringRef,
};
use objc2::{
    AnyThread, ClassType,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
};
use objc2_cloud_kit::{
    CKAccountStatus, CKAsset, CKContainer, CKCurrentUserDefaultName, CKDatabase, CKErrorCode,
    CKFetchRecordZoneChangesConfiguration, CKFetchRecordZoneChangesOperation,
    CKModifyRecordZonesOperation, CKModifyRecordsOperation, CKRecord, CKRecordID,
    CKRecordNameZoneWideShare, CKRecordSavePolicy, CKRecordType, CKRecordValue, CKRecordZone,
    CKRecordZoneID, CKServerChangeToken, CKShare, CKShareMetadata, CKShareParticipantPermission,
    CKShareTitleKey,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSKeyedArchiver, NSKeyedUnarchiver, NSNumber, NSString,
    NSURL,
};
use paste_sync::{SyncBlob, SyncEnvelope, SyncPayload, SyncScope};

use crate::{
    CloudDownloadBatch, CloudKitAccountStatus, CloudKitConfiguration, CloudKitError,
    CloudRecordPlan, CloudRecordValue, CloudShareDescriptor, CloudSharePermission, CloudSharePlan,
    CloudUploadPlan, CloudUploadReceipt,
};

const FETCH_PAGE_LIMIT: usize = 200;
const MAX_DOWNLOAD_BLOBS: usize = 2_048;
const MAX_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_DOWNLOAD_ASSET_BYTES: u64 = 64 * 1024 * 1024;

type SecTaskRef = *const std::ffi::c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecTaskCreateFromSelf(allocator: CFAllocatorRef) -> SecTaskRef;
    fn SecTaskCopyValueForEntitlement(
        task: SecTaskRef,
        entitlement: CFStringRef,
        error: *mut CFErrorRef,
    ) -> CFTypeRef;
}

struct OwnedSecTask(SecTaskRef);

impl Drop for OwnedSecTask {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: SecTaskCreateFromSelf follows the Core Foundation create
            // rule and this guard owns exactly one reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

pub(super) fn has_required_entitlements(configuration: &CloudKitConfiguration) -> bool {
    // SAFETY: A null allocator selects the default allocator. The returned task
    // follows the create rule and is released by OwnedSecTask.
    let task = OwnedSecTask(unsafe { SecTaskCreateFromSelf(std::ptr::null()) });
    if task.0.is_null() {
        return false;
    }
    entitlement_array_contains(&task, "com.apple.developer.icloud-services", "CloudKit")
        && entitlement_array_contains(
            &task,
            "com.apple.developer.icloud-container-identifiers",
            configuration.container_identifier(),
        )
}

fn entitlement_array_contains(task: &OwnedSecTask, entitlement: &str, expected: &str) -> bool {
    let entitlement = CFString::new(entitlement);
    // SAFETY: The SecTask and CFString are live. Passing a null error-out
    // pointer is permitted; a non-null result follows the copy rule.
    let value = unsafe {
        SecTaskCopyValueForEntitlement(
            task.0,
            entitlement.as_concrete_TypeRef(),
            std::ptr::null_mut(),
        )
    };
    if value.is_null() {
        return false;
    }
    // SAFETY: SecTaskCopyValueForEntitlement returned an owned CF object.
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    let Some(array) = value.downcast::<CFArray>() else {
        return false;
    };
    array.iter().any(|item| {
        let pointer = *item;
        if pointer.is_null() {
            return false;
        }
        // SAFETY: CFArray retains each element and the array remains live while
        // this borrowed wrapper performs a runtime-checked string downcast.
        let value = unsafe { CFType::wrap_under_get_rule(pointer) };
        value
            .downcast::<CFString>()
            .is_some_and(|value| value == expected)
    })
}

pub(super) fn account_status(
    configuration: &CloudKitConfiguration,
    timeout: Duration,
) -> Result<CloudKitAccountStatus, CloudKitError> {
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: `identifier` is a live NSString for the duration of this call.
    // CloudKit returns an owned container and the generated binding models that
    // ownership as `Retained<CKContainer>`.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |status: CKAccountStatus, error: *mut NSError| {
        let _ = sender.send((status, error.is_null()));
    });
    // SAFETY: `completion` is heap allocated, remains alive while this function
    // waits, captures only a thread-safe channel sender, and CloudKit invokes
    // account-status handlers on its own serial queue.
    unsafe { container.accountStatusWithCompletionHandler(&completion) };

    match receiver.recv_timeout(timeout) {
        Ok((_, false)) => Err(CloudKitError::AccountStatusRequestFailed),
        Ok((status, true)) => Ok(map_account_status(status)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::AccountStatusTimeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CloudKitError::AccountStatusRequestFailed),
    }
}

pub(super) fn create_pinboard_share(
    configuration: &CloudKitConfiguration,
    plan: &CloudSharePlan,
    timeout: Duration,
) -> Result<CloudShareDescriptor, CloudKitError> {
    let deadline = Instant::now() + timeout;
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The identifier is a live NSString and CloudKit returns a retained
    // container whose private database is valid for the operation lifetime.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    let database = unsafe { container.privateCloudDatabase() };
    ensure_private_zone_named(
        &database,
        plan.zone_name(),
        remaining(deadline, CloudKitError::ZonePreparationTimeout)?,
    )?;

    let zone_name = NSString::from_str(plan.zone_name());
    // SAFETY: The built-in current-user owner name and validated zone name are
    // live for initialization; the returned zone ID is retained.
    let zone_id = unsafe {
        CKRecordZoneID::initWithZoneName_ownerName(
            CKRecordZoneID::alloc(),
            &zone_name,
            CKCurrentUserDefaultName,
        )
    };
    // SAFETY: Zone-wide CKShare is the CloudKit-supported isolation primitive
    // for sharing all records of exactly one Pinboard zone.
    let share = unsafe { CKShare::initWithRecordZoneID(CKShare::alloc(), &zone_id) };
    let permission = match plan.permission() {
        CloudSharePermission::ReadOnly => CKShareParticipantPermission::ReadOnly,
        CloudSharePermission::ReadWrite => CKShareParticipantPermission::ReadWrite,
    };
    // SAFETY: The share is exclusively configured before submission and the
    // enum values come directly from CloudKit's generated binding.
    unsafe { share.setPublicPermission(permission) };
    set_record_string_field(&share, "pinboardID", &plan.pinboard_id().to_string());
    set_record_string_field(&share, "pinboardTitle", plan.title());
    set_record_string_field(&share, "pinboardColor", plan.color());
    let title: Retained<ProtocolObject<dyn CKRecordValue>> =
        ProtocolObject::from_retained(NSString::from_str(plan.title()));
    // SAFETY: NSString conforms to CKRecordValue; the standard title key is a
    // CloudKit-owned NSString and the share copies the value.
    unsafe { share.setObject_forKey(Some(&title), CKShareTitleKey) };

    let share_record: Retained<CKRecord> = Retained::clone(&share).into_super();
    let records = NSArray::from_retained_slice(&[share_record]);
    // SAFETY: No deletions are included; the zone-wide share is saved as the
    // only record in an atomic operation.
    let operation = unsafe {
        CKModifyRecordsOperation::initWithRecordsToSave_recordIDsToDelete(
            CKModifyRecordsOperation::alloc(),
            Some(&records),
            None,
        )
    };
    unsafe {
        operation.setSavePolicy(CKRecordSavePolicy::AllKeys);
        operation.setAtomic(true);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(
        move |saved_records: *mut NSArray<CKRecord>,
              _deleted_ids: *mut NSArray<CKRecordID>,
              error: *mut NSError| {
            let result = if error.is_null() {
                parse_saved_share(saved_records, None)
            } else {
                Err(CloudKitError::ShareCreationFailed)
            };
            let _ = sender.send(result);
        },
    );
    // SAFETY: The operation copies the heap block, which captures only a Send
    // channel; it is fully configured before submission.
    unsafe {
        operation.setModifyRecordsCompletionBlock(Some(&completion));
        database.addOperation(&operation);
    }
    match receiver.recv_timeout(remaining(deadline, CloudKitError::ShareCreationTimeout)?) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CloudKitError::ShareCreationFailed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::ShareCreationTimeout),
    }
}

pub(super) fn accept_pinboard_share(
    configuration: &CloudKitConfiguration,
    invitation_url: &str,
    timeout: Duration,
) -> Result<CloudShareDescriptor, CloudKitError> {
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The identifier is valid for construction and the returned
    // container is retained by this function and the callback block.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    let url = NSURL::URLWithString(&NSString::from_str(invitation_url))
        .ok_or(CloudKitError::InvalidShareInvitationUrl)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let accept_container = Retained::clone(&container);
    let expected_container_identifier = configuration.container_identifier().to_owned();
    let fetch_sender = sender.clone();
    let fetched = RcBlock::new(move |metadata: *mut CKShareMetadata, error: *mut NSError| {
        let Some(metadata) = NonNull::new(metadata) else {
            let _ = fetch_sender.send(Err(CloudKitError::ShareAcceptanceFailed));
            return;
        };
        if !error.is_null() {
            let _ = fetch_sender.send(Err(CloudKitError::ShareAcceptanceFailed));
            return;
        }
        // SAFETY: CloudKit guarantees metadata remains live for the callback;
        // all descriptor fields are copied before returning.
        let metadata = unsafe { metadata.as_ref() };
        let actual_container_identifier = unsafe { metadata.containerIdentifier() }.to_string();
        if actual_container_identifier != expected_container_identifier {
            let _ = fetch_sender.send(Err(CloudKitError::InvalidShareMetadata));
            return;
        }
        let permission = unsafe { metadata.participantPermission() };
        let share = unsafe { metadata.share() };
        let descriptor = match descriptor_from_share(&share, Some(permission)) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                let _ = fetch_sender.send(Err(error));
                return;
            }
        };
        let accepted_sender = fetch_sender.clone();
        let accepted = RcBlock::new(move |share: *mut CKShare, error: *mut NSError| {
            let result = if error.is_null() && !share.is_null() {
                Ok(descriptor.clone())
            } else {
                Err(CloudKitError::ShareAcceptanceFailed)
            };
            let _ = accepted_sender.send(result);
        });
        // SAFETY: Metadata and the retained container are live. CloudKit copies
        // the nested heap block before the fetch callback returns.
        unsafe { accept_container.acceptShareMetadata_completionHandler(metadata, &accepted) };
    });
    // SAFETY: The validated URL and retained container are live and CloudKit
    // copies the completion block.
    unsafe { container.fetchShareMetadataWithURL_completionHandler(&url, &fetched) };
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CloudKitError::ShareAcceptanceFailed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::ShareAcceptanceTimeout),
    }
}

fn parse_saved_share(
    records: *mut NSArray<CKRecord>,
    participant_permission: Option<CKShareParticipantPermission>,
) -> Result<CloudShareDescriptor, CloudKitError> {
    let records = NonNull::new(records).ok_or(CloudKitError::ShareCreationFailed)?;
    // SAFETY: The CloudKit completion callback guarantees the NSArray remains
    // valid for the callback duration. `iter` retains the selected object.
    let records = unsafe { records.as_ref() };
    if records.count() != 1 {
        return Err(CloudKitError::ShareCreationFailed);
    }
    let record = records.objectAtIndex(0);
    let share = record
        .downcast::<CKShare>()
        .map_err(|_| CloudKitError::ShareCreationFailed)?;
    descriptor_from_share(&share, participant_permission)
}

fn descriptor_from_share(
    share: &CKShare,
    participant_permission: Option<CKShareParticipantPermission>,
) -> Result<CloudShareDescriptor, CloudKitError> {
    let pinboard_id = uuid::Uuid::parse_str(&record_string_field(share, "pinboardID")?)
        .map_err(|_| CloudKitError::InvalidShareMetadata)?;
    let title = record_string_field(share, "pinboardTitle")?;
    let color = record_string_field(share, "pinboardColor")?.to_ascii_lowercase();
    if title.is_empty()
        || title.chars().count() > 80
        || color.len() != 7
        || !color.starts_with('#')
        || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CloudKitError::InvalidShareMetadata);
    }
    // SAFETY: The saved CKShare is retained; the returned ID and zone ID are
    // retained by generated bindings before their strings are copied.
    let record_id = unsafe { share.recordID() };
    let zone_id = unsafe { record_id.zoneID() };
    let zone_name = unsafe { zone_id.zoneName() }.to_string();
    let owner_name = unsafe { zone_id.ownerName() }.to_string();
    let share_record_name = unsafe { record_id.recordName() }.to_string();
    // SAFETY: CloudKit exports this process-lifetime NSString constant.
    let zone_wide_share_record_name = unsafe { CKRecordNameZoneWideShare }.to_string();
    if zone_name != format!("PasteShare_{pinboard_id}")
        || owner_name.is_empty()
        || share_record_name != zone_wide_share_record_name
    {
        return Err(CloudKitError::InvalidShareMetadata);
    }
    let url = unsafe { share.URL() }.ok_or(CloudKitError::InvalidShareMetadata)?;
    let share_url = url
        .absoluteString()
        .ok_or(CloudKitError::InvalidShareMetadata)?
        .to_string();
    crate::validate_invitation_url(&share_url)?;
    let native_permission = participant_permission
        .filter(|permission| *permission != CKShareParticipantPermission::Unknown)
        .unwrap_or_else(|| unsafe { share.publicPermission() });
    let permission = match native_permission {
        CKShareParticipantPermission::ReadOnly => CloudSharePermission::ReadOnly,
        CKShareParticipantPermission::ReadWrite => CloudSharePermission::ReadWrite,
        _ => return Err(CloudKitError::InvalidShareMetadata),
    };
    Ok(CloudShareDescriptor {
        pinboard_id,
        title,
        color,
        zone_name,
        owner_name,
        share_record_name,
        share_url,
        permission,
    })
}

fn record_string_field(record: &CKRecord, key: &str) -> Result<String, CloudKitError> {
    let key = NSString::from_str(key);
    // SAFETY: The record and key are live; downcast verifies the required
    // Foundation type before any string is copied.
    let value = unsafe { record.objectForKey(&key) }.ok_or(CloudKitError::InvalidShareMetadata)?;
    value
        .downcast::<NSString>()
        .map(|value| value.to_string())
        .map_err(|_| CloudKitError::InvalidShareMetadata)
}

fn set_record_string_field(record: &CKRecord, key: &str, value: &str) {
    set_record_field(record, key, &CloudRecordValue::String(value.to_owned()));
}

pub(super) fn upload_private(
    configuration: &CloudKitConfiguration,
    plan: &CloudUploadPlan,
    timeout: Duration,
) -> Result<CloudUploadReceipt, CloudKitError> {
    let deadline = Instant::now() + timeout;
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The NSString is live during the constructor and the binding
    // returns a retained container.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    // SAFETY: The retained container is not mutated concurrently in this
    // function; CloudKit owns its internal scheduling.
    let database = unsafe { container.privateCloudDatabase() };
    ensure_private_zone_named(
        &database,
        "PastePrivate",
        remaining(deadline, CloudKitError::ZonePreparationTimeout)?,
    )?;

    upload_plan_to_database(
        &database,
        plan,
        remaining(deadline, CloudKitError::UploadTimeout)?,
    )
}

pub(super) fn upload_pinboard_share(
    configuration: &CloudKitConfiguration,
    plan: &CloudUploadPlan,
    role: crate::CloudShareRole,
    timeout: Duration,
) -> Result<CloudUploadReceipt, CloudKitError> {
    let deadline = Instant::now() + timeout;
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The identifier is live and the container/database returned by
    // CloudKit remain retained for the operation lifetime.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    let database = match role {
        crate::CloudShareRole::Owner => unsafe { container.privateCloudDatabase() },
        crate::CloudShareRole::Participant => unsafe { container.sharedCloudDatabase() },
    };
    if role == crate::CloudShareRole::Owner {
        ensure_private_zone_named(
            &database,
            &plan.records[0].zone_name,
            remaining(deadline, CloudKitError::ZonePreparationTimeout)?,
        )?;
    }
    upload_plan_to_database(
        &database,
        plan,
        remaining(deadline, CloudKitError::UploadTimeout)?,
    )
}

fn upload_plan_to_database(
    database: &CKDatabase,
    plan: &CloudUploadPlan,
    timeout: Duration,
) -> Result<CloudUploadReceipt, CloudKitError> {
    let staging_directory = create_asset_staging_directory()?;
    let native_records = match build_records(plan, &staging_directory) {
        Ok(records) => records,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_directory);
            return Err(error);
        }
    };
    let native_records = NSArray::from_retained_slice(&native_records);
    // SAFETY: Both arrays are valid for the constructor; no record deletions
    // are used because sync deletions are durable tombstone records.
    let operation = unsafe {
        CKModifyRecordsOperation::initWithRecordsToSave_recordIDsToDelete(
            CKModifyRecordsOperation::alloc(),
            Some(&native_records),
            None,
        )
    };
    // `AllKeys` is safe here because operation IDs and blob hashes are
    // immutable, content-addressed record names. Retrying can only write the
    // same validated bytes. Atomicity prevents a partially acknowledged batch.
    unsafe {
        operation.setSavePolicy(CKRecordSavePolicy::AllKeys);
        operation.setAtomic(true);
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    let cleanup_directory = staging_directory.clone();
    let completion = RcBlock::new(
        move |_saved_records: *mut NSArray<CKRecord>,
              _deleted_ids: *mut NSArray<CKRecordID>,
              error: *mut NSError| {
            let _ = fs::remove_dir_all(&cleanup_directory);
            let _ = sender.send(error.is_null());
        },
    );
    // SAFETY: CloudKit copies the heap block. Its captured sender and path are
    // Send, it accepts the documented nullable completion arguments, and the
    // operation is fully configured before being submitted.
    unsafe {
        operation.setModifyRecordsCompletionBlock(Some(&completion));
        database.addOperation(&operation);
    }

    match receiver.recv_timeout(timeout) {
        Ok(true) => Ok(CloudUploadReceipt {
            saved_record_count: plan.records.len(),
            acknowledge_operation_ids: plan.acknowledge_operation_ids.clone(),
        }),
        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(CloudKitError::UploadFailed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::UploadTimeout),
    }
}

pub(super) fn fetch_private_changes(
    configuration: &CloudKitConfiguration,
    previous_server_change_token: Option<&[u8]>,
    timeout: Duration,
) -> Result<CloudDownloadBatch, CloudKitError> {
    let deadline = Instant::now() + timeout;
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The NSString is live during construction and CloudKit returns a
    // retained container.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    // SAFETY: The retained container is not mutated concurrently here.
    let database = unsafe { container.privateCloudDatabase() };
    ensure_private_zone_named(
        &database,
        "PastePrivate",
        remaining(deadline, CloudKitError::ZonePreparationTimeout)?,
    )?;
    fetch_zone_changes(
        &database,
        "PastePrivate",
        None,
        SyncScope::Private,
        previous_server_change_token,
        remaining(deadline, CloudKitError::DownloadTimeout)?,
    )
}

pub(super) fn fetch_pinboard_share_changes(
    configuration: &CloudKitConfiguration,
    zone_name: &str,
    owner_name: &str,
    role: crate::CloudShareRole,
    previous_server_change_token: Option<&[u8]>,
    timeout: Duration,
) -> Result<CloudDownloadBatch, CloudKitError> {
    let deadline = Instant::now() + timeout;
    let identifier = NSString::from_str(configuration.container_identifier());
    // SAFETY: The identifier is live and CloudKit returns retained database
    // handles appropriate to the caller's share role.
    let container = unsafe { CKContainer::containerWithIdentifier(&identifier) };
    let database = match role {
        crate::CloudShareRole::Owner => unsafe { container.privateCloudDatabase() },
        crate::CloudShareRole::Participant => unsafe { container.sharedCloudDatabase() },
    };
    if role == crate::CloudShareRole::Owner {
        ensure_private_zone_named(
            &database,
            zone_name,
            remaining(deadline, CloudKitError::ZonePreparationTimeout)?,
        )?;
    }
    fetch_zone_changes(
        &database,
        zone_name,
        Some(owner_name),
        SyncScope::Shared,
        previous_server_change_token,
        remaining(deadline, CloudKitError::DownloadTimeout)?,
    )
}

fn fetch_zone_changes(
    database: &CKDatabase,
    zone_name: &str,
    owner_name: Option<&str>,
    expected_scope: SyncScope,
    previous_server_change_token: Option<&[u8]>,
    timeout: Duration,
) -> Result<CloudDownloadBatch, CloudKitError> {
    let zone_name = NSString::from_str(zone_name);
    let zone_id = if let Some(owner_name) = owner_name {
        let owner_name = NSString::from_str(owner_name);
        // SAFETY: Both identifiers are validated and live for initialization.
        unsafe {
            CKRecordZoneID::initWithZoneName_ownerName(
                CKRecordZoneID::alloc(),
                &zone_name,
                &owner_name,
            )
        }
    } else {
        // SAFETY: The NSString is live during initialization and zoneID is
        // retained by the generated bindings.
        let zone = unsafe { CKRecordZone::initWithZoneName(CKRecordZone::alloc(), &zone_name) };
        unsafe { zone.zoneID() }
    };
    let zone_ids = NSArray::from_retained_slice(std::slice::from_ref(&zone_id));
    // SAFETY: The generated constructor returns an owned configuration.
    let fetch_configuration = unsafe { CKFetchRecordZoneChangesConfiguration::new() };
    // SAFETY: The configuration is exclusively owned until submitted.
    unsafe { fetch_configuration.setResultsLimit(FETCH_PAGE_LIMIT) };
    let previous_token = previous_server_change_token
        .map(decode_server_change_token)
        .transpose()?;
    if let Some(previous_token) = previous_token.as_ref() {
        // SAFETY: The token is retained and copied by the configuration.
        unsafe { fetch_configuration.setPreviousServerChangeToken(Some(previous_token)) };
    }
    let configurations: Retained<
        NSDictionary<CKRecordZoneID, CKFetchRecordZoneChangesConfiguration>,
    > = NSDictionary::from_slices(&[&*zone_id], &[&*fetch_configuration]);
    // SAFETY: The zone and configuration collections are valid and copied by
    // the operation.
    let operation = unsafe {
        CKFetchRecordZoneChangesOperation::initWithRecordZoneIDs_configurationsByRecordZoneID(
            CKFetchRecordZoneChangesOperation::alloc(),
            &zone_ids,
            Some(&configurations),
        )
    };
    // One page keeps memory bounded. The returned token resumes the next page.
    unsafe { operation.setFetchAllChanges(false) };

    let accumulator = Arc::new(Mutex::new(DownloadAccumulator::default()));
    let record_accumulator = Arc::clone(&accumulator);
    let record_changed = RcBlock::new(
        move |_record_id: NonNull<CKRecordID>, record: *mut CKRecord, error: *mut NSError| {
            if !error.is_null() {
                set_download_error(&record_accumulator, CloudKitError::DownloadFailed);
                return;
            }
            let Some(record) = NonNull::new(record) else {
                set_download_error(&record_accumulator, CloudKitError::InvalidCloudRecord);
                return;
            };
            // SAFETY: CloudKit documents a valid record pointer when the error
            // is nil. Parsing copies all Rust data before the callback returns.
            let parsed = unsafe { parse_changed_record(record.as_ref(), expected_scope) };
            match parsed {
                Ok(Some(parsed)) => append_download_record(&record_accumulator, parsed),
                Ok(None) => {}
                Err(error) => set_download_error(&record_accumulator, error),
            }
        },
    );
    let deletion_accumulator = Arc::clone(&accumulator);
    let record_deleted = RcBlock::new(
        move |_record_id: NonNull<CKRecordID>, _record_type: NonNull<CKRecordType>| {
            set_download_error(
                &deletion_accumulator,
                CloudKitError::UnexpectedCloudRecordDeletion,
            );
        },
    );
    let token_accumulator = Arc::clone(&accumulator);
    let zone_completed = RcBlock::new(
        move |_zone_id: NonNull<CKRecordZoneID>,
              token: *mut CKServerChangeToken,
              _client_token: *mut NSData,
              more_coming: Bool,
              error: *mut NSError| {
            if !error.is_null() {
                set_download_error(&token_accumulator, classify_download_error(error));
                return;
            }
            let Some(token) = NonNull::new(token) else {
                set_download_error(&token_accumulator, CloudKitError::InvalidServerChangeToken);
                return;
            };
            // SAFETY: CloudKit documents the token pointer as valid or null;
            // the non-null token is archived before this callback returns.
            match unsafe { archive_server_change_token(token.as_ref()) } {
                Ok(token) => {
                    if let Ok(mut accumulator) = token_accumulator.lock() {
                        accumulator.server_change_token = Some(token);
                        accumulator.more_coming = more_coming.as_bool();
                    }
                }
                Err(error) => set_download_error(&token_accumulator, error),
            }
        },
    );
    let completion_accumulator = Arc::clone(&accumulator);
    let (sender, receiver) = mpsc::sync_channel(1);
    let completed = RcBlock::new(move |error: *mut NSError| {
        if !error.is_null() {
            set_download_error(&completion_accumulator, classify_download_error(error));
        }
        let _ = sender.send(error.is_null());
    });
    // SAFETY: All blocks are heap allocated, copied by the operation, and
    // capture only Send synchronization primitives. Their pointer signatures
    // exactly match the generated CloudKit callback contracts.
    unsafe {
        operation.setRecordWasChangedBlock(Some(&record_changed));
        operation.setRecordWithIDWasDeletedBlock(Some(&record_deleted));
        operation.setRecordZoneFetchCompletionBlock(Some(&zone_completed));
        operation.setFetchRecordZoneChangesCompletionBlock(Some(&completed));
        database.addOperation(&operation);
    }

    match receiver.recv_timeout(timeout) {
        Ok(true) => finish_download(accumulator),
        Ok(false) => match finish_download(accumulator) {
            Err(error) => Err(error),
            Ok(_) => Err(CloudKitError::DownloadFailed),
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CloudKitError::DownloadFailed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::DownloadTimeout),
    }
}

fn classify_download_error(error: *mut NSError) -> CloudKitError {
    let Some(error) = NonNull::new(error) else {
        return CloudKitError::DownloadFailed;
    };
    // SAFETY: CloudKit callback contracts guarantee a non-null NSError pointer
    // remains valid for the callback duration.
    let code = unsafe { error.as_ref().code() };
    if code == CKErrorCode::ChangeTokenExpired.0 {
        CloudKitError::ServerChangeTokenExpired
    } else {
        CloudKitError::DownloadFailed
    }
}

#[derive(Default)]
struct DownloadAccumulator {
    envelopes: Vec<SyncEnvelope>,
    blobs: BTreeMap<[u8; 32], Vec<u8>>,
    server_change_token: Option<Vec<u8>>,
    more_coming: bool,
    total_bytes: usize,
    error: Option<CloudKitError>,
}

struct ParsedCloudRecord {
    envelope: SyncEnvelope,
    blobs: Vec<SyncBlob>,
    byte_len: usize,
}

unsafe fn parse_changed_record(
    record: &CKRecord,
    expected_scope: SyncScope,
) -> Result<Option<ParsedCloudRecord>, CloudKitError> {
    // SAFETY: The callback guarantees a live CKRecord for the duration of this
    // function; returned Objective-C values are retained by the bindings.
    let record_type = unsafe { record.recordType() };
    if record_type.to_string() != "PasteChange" {
        return Ok(None);
    }
    let envelope_data = record_data_field(record, "envelope")?;
    if envelope_data.len() > crate::MAX_ENVELOPE_BYTES {
        return Err(CloudKitError::DownloadTooLarge);
    }
    let envelope: SyncEnvelope = serde_json::from_slice(&envelope_data.to_vec())
        .map_err(|_| CloudKitError::InvalidCloudRecord)?;
    envelope
        .validate()
        .map_err(|_| CloudKitError::InvalidCloudRecord)?;
    // SAFETY: The record and its retained identifier are live during parsing.
    let record_name = unsafe { record.recordID().recordName() }.to_string();
    if envelope.scope != expected_scope
        || record_name != crate::change_record_name(envelope.change.operation_id)
    {
        return Err(CloudKitError::InvalidCloudRecord);
    }

    let mut byte_len = envelope_data.len();
    let mut blobs = Vec::new();
    let mut seen = BTreeMap::new();
    if let Some(SyncPayload::Clip(clip)) = envelope.payload.as_ref() {
        for representation in &clip.representations {
            if seen.contains_key(&representation.content_hash) {
                continue;
            }
            let field_name = format!("asset_{}", crate::hex_hash(&representation.content_hash));
            let asset = record_asset_field(record, &field_name)?;
            // SAFETY: CKAsset owns the file URL for the fetched record lifetime
            // and the bytes are copied before this callback returns.
            let file_url = unsafe { asset.fileURL() }.ok_or(CloudKitError::InvalidCloudRecord)?;
            let path = file_url
                .to_file_path()
                .ok_or(CloudKitError::InvalidCloudRecord)?;
            let metadata = fs::metadata(&path).map_err(|_| CloudKitError::InvalidCloudRecord)?;
            if metadata.len() > MAX_DOWNLOAD_ASSET_BYTES {
                return Err(CloudKitError::DownloadTooLarge);
            }
            let bytes = fs::read(&path).map_err(|_| CloudKitError::InvalidCloudRecord)?;
            if *blake3::hash(&bytes).as_bytes() != representation.content_hash {
                return Err(CloudKitError::InvalidCloudRecord);
            }
            byte_len = byte_len
                .checked_add(bytes.len())
                .ok_or(CloudKitError::DownloadTooLarge)?;
            seen.insert(representation.content_hash, ());
            blobs.push(SyncBlob {
                content_hash: representation.content_hash,
                bytes,
            });
        }
    }
    Ok(Some(ParsedCloudRecord {
        envelope,
        blobs,
        byte_len,
    }))
}

fn record_data_field(record: &CKRecord, key: &str) -> Result<Retained<NSData>, CloudKitError> {
    let key = NSString::from_str(key);
    // SAFETY: `record` and key are valid objects. The protocol object is
    // retained by the binding before the runtime-checked downcast.
    let value = unsafe { record.objectForKey(&key) }.ok_or(CloudKitError::InvalidCloudRecord)?;
    value
        .downcast::<NSData>()
        .map_err(|_| CloudKitError::InvalidCloudRecord)
}

fn record_asset_field(record: &CKRecord, key: &str) -> Result<Retained<CKAsset>, CloudKitError> {
    let key = NSString::from_str(key);
    // SAFETY: See `record_data_field`; the downcast verifies CKAsset.
    let value = unsafe { record.objectForKey(&key) }.ok_or(CloudKitError::InvalidCloudRecord)?;
    value
        .downcast::<CKAsset>()
        .map_err(|_| CloudKitError::InvalidCloudRecord)
}

fn append_download_record(
    accumulator: &Arc<Mutex<DownloadAccumulator>>,
    parsed: ParsedCloudRecord,
) {
    let Ok(mut accumulator) = accumulator.lock() else {
        return;
    };
    if accumulator.error.is_some() {
        return;
    }
    let Some(total_bytes) = accumulator.total_bytes.checked_add(parsed.byte_len) else {
        accumulator.error = Some(CloudKitError::DownloadTooLarge);
        return;
    };
    if total_bytes > MAX_DOWNLOAD_BYTES || accumulator.envelopes.len() >= FETCH_PAGE_LIMIT {
        accumulator.error = Some(CloudKitError::DownloadTooLarge);
        return;
    }
    for blob in parsed.blobs {
        if let Some(existing) = accumulator.blobs.get(&blob.content_hash)
            && existing != &blob.bytes
        {
            accumulator.error = Some(CloudKitError::InvalidCloudRecord);
            return;
        }
        accumulator
            .blobs
            .entry(blob.content_hash)
            .or_insert(blob.bytes);
        if accumulator.blobs.len() > MAX_DOWNLOAD_BLOBS {
            accumulator.error = Some(CloudKitError::DownloadTooLarge);
            return;
        }
    }
    accumulator.total_bytes = total_bytes;
    accumulator.envelopes.push(parsed.envelope);
}

fn set_download_error(accumulator: &Arc<Mutex<DownloadAccumulator>>, error: CloudKitError) {
    if let Ok(mut accumulator) = accumulator.lock()
        && accumulator.error.is_none()
    {
        accumulator.error = Some(error);
    }
}

fn finish_download(
    accumulator: Arc<Mutex<DownloadAccumulator>>,
) -> Result<CloudDownloadBatch, CloudKitError> {
    let mut accumulator = accumulator
        .lock()
        .map_err(|_| CloudKitError::DownloadFailed)?;
    if let Some(error) = accumulator.error.take() {
        return Err(error);
    }
    let server_change_token = accumulator
        .server_change_token
        .take()
        .ok_or(CloudKitError::InvalidServerChangeToken)?;
    Ok(CloudDownloadBatch {
        envelopes: std::mem::take(&mut accumulator.envelopes),
        blobs: std::mem::take(&mut accumulator.blobs)
            .into_iter()
            .map(|(content_hash, bytes)| SyncBlob {
                content_hash,
                bytes,
            })
            .collect(),
        server_change_token,
        more_coming: accumulator.more_coming,
    })
}

fn decode_server_change_token(
    bytes: &[u8],
) -> Result<Retained<CKServerChangeToken>, CloudKitError> {
    let data = NSData::with_bytes(bytes);
    // SAFETY: Secure decoding is restricted to CKServerChangeToken and NSData
    // owns the input bytes for the duration of the call.
    let object = unsafe {
        NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(
            CKServerChangeToken::class(),
            &data,
        )
    }
    .map_err(|_| CloudKitError::InvalidServerChangeToken)?;
    object
        .downcast::<CKServerChangeToken>()
        .map_err(|_| CloudKitError::InvalidServerChangeToken)
}

unsafe fn archive_server_change_token(
    token: &CKServerChangeToken,
) -> Result<Vec<u8>, CloudKitError> {
    // SAFETY: CKServerChangeToken conforms to NSSecureCoding and is live for
    // the duration of the secure archive call.
    let data = unsafe {
        NSKeyedArchiver::archivedDataWithRootObject_requiringSecureCoding_error(token, true)
    }
    .map_err(|_| CloudKitError::ServerChangeTokenArchiveFailed)?;
    Ok(data.to_vec())
}

fn ensure_private_zone_named(
    database: &CKDatabase,
    name: &str,
    timeout: Duration,
) -> Result<(), CloudKitError> {
    let zone_name = NSString::from_str(name);
    // SAFETY: The NSString is live for the initializer and the result is
    // retained by the Rust owner and subsequently by the NSArray.
    let zone = unsafe { CKRecordZone::initWithZoneName(CKRecordZone::alloc(), &zone_name) };
    let zones = NSArray::from_retained_slice(&[zone]);
    // SAFETY: The save array is valid and copied by the configured operation.
    let operation = unsafe {
        CKModifyRecordZonesOperation::initWithRecordZonesToSave_recordZoneIDsToDelete(
            CKModifyRecordZonesOperation::alloc(),
            Some(&zones),
            None,
        )
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(
        move |_saved_zones: *mut NSArray<CKRecordZone>,
              _deleted_zone_ids: *mut NSArray<CKRecordZoneID>,
              error: *mut NSError| {
            let _ = sender.send(error.is_null());
        },
    );
    // SAFETY: The copied completion block captures only a Send channel, and
    // the operation is fully configured before submission.
    unsafe {
        operation.setModifyRecordZonesCompletionBlock(Some(&completion));
        database.addOperation(&operation);
    }
    match receiver.recv_timeout(timeout) {
        Ok(true) => Ok(()),
        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(CloudKitError::ZonePreparationFailed)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CloudKitError::ZonePreparationTimeout),
    }
}

fn build_records(
    plan: &CloudUploadPlan,
    staging_directory: &Path,
) -> Result<Vec<Retained<CKRecord>>, CloudKitError> {
    plan.records
        .iter()
        .enumerate()
        .map(|(index, plan)| build_record(plan, staging_directory, index))
        .collect()
}

#[cfg(test)]
pub(super) fn round_trip_record_plan(
    plan: &CloudRecordPlan,
    expected_scope: SyncScope,
) -> Result<(SyncEnvelope, Vec<SyncBlob>), CloudKitError> {
    let staging_directory = create_asset_staging_directory()?;
    let result = (|| {
        let record = build_record(plan, &staging_directory, 0)?;
        // SAFETY: `record` is retained for the entire parse and was constructed
        // using the same validated CKRecord schema as the upload path.
        let parsed = unsafe { parse_changed_record(&record, expected_scope) }?
            .ok_or(CloudKitError::InvalidCloudRecord)?;
        Ok((parsed.envelope, parsed.blobs))
    })();
    let _ = fs::remove_dir_all(staging_directory);
    result
}

fn build_record(
    plan: &CloudRecordPlan,
    staging_directory: &Path,
    index: usize,
) -> Result<Retained<CKRecord>, CloudKitError> {
    let zone_name = NSString::from_str(&plan.zone_name);
    let zone_id = if let Some(owner_name) = plan.zone_owner_name.as_deref() {
        let owner_name = NSString::from_str(owner_name);
        // SAFETY: Both validated strings are live for initialization and the
        // resulting shared-zone ID retains their values.
        unsafe {
            CKRecordZoneID::initWithZoneName_ownerName(
                CKRecordZoneID::alloc(),
                &zone_name,
                &owner_name,
            )
        }
    } else {
        // SAFETY: The zone name is validated and live for the initializer. The
        // convenience initializer creates the current-user zone ID.
        let zone = unsafe { CKRecordZone::initWithZoneName(CKRecordZone::alloc(), &zone_name) };
        // SAFETY: The zone is retained for this call and returns a retained ID.
        unsafe { zone.zoneID() }
    };
    let record_name = NSString::from_str(&plan.record_name);
    // SAFETY: Both NSString and zone ID are live for the initializer.
    let record_id = unsafe {
        CKRecordID::initWithRecordName_zoneID(CKRecordID::alloc(), &record_name, &zone_id)
    };
    let record_type = NSString::from_str(plan.record_type);
    // SAFETY: The record type and ID are live for the initializer; the returned
    // record retains its ID.
    let record = unsafe {
        CKRecord::initWithRecordType_recordID(CKRecord::alloc(), &record_type, &record_id)
    };
    for (key, value) in &plan.fields {
        set_record_field(&record, key, value);
    }
    for (asset_index, asset) in plan.assets.iter().enumerate() {
        let asset_path = staging_directory.join(format!("{index}-{asset_index}.asset"));
        write_protected(&asset_path, &asset.bytes)?;
        let file_url =
            NSURL::from_file_path(&asset_path).ok_or(CloudKitError::AssetStagingFailed)?;
        // SAFETY: The file URL is valid and remains backed by a file until the
        // operation completion block runs.
        let native_asset = unsafe { CKAsset::initWithFileURL(CKAsset::alloc(), &file_url) };
        let native_asset: Retained<ProtocolObject<dyn CKRecordValue>> =
            ProtocolObject::from_retained(native_asset);
        let key = NSString::from_str(&asset.field_name);
        // SAFETY: CKAsset conforms to CKRecordValue and both objects are live.
        unsafe { record.setObject_forKey(Some(&native_asset), &key) };
    }
    Ok(record)
}

fn set_record_field(record: &CKRecord, key: &str, value: &CloudRecordValue) {
    let key = NSString::from_str(key);
    let value: Retained<ProtocolObject<dyn CKRecordValue>> = match value {
        CloudRecordValue::String(value) => ProtocolObject::from_retained(NSString::from_str(value)),
        CloudRecordValue::Integer(value) => {
            ProtocolObject::from_retained(NSNumber::new_i64(*value))
        }
        CloudRecordValue::Boolean(value) => {
            ProtocolObject::from_retained(NSNumber::new_bool(*value))
        }
        CloudRecordValue::Bytes(value) => ProtocolObject::from_retained(NSData::with_bytes(value)),
    };
    // SAFETY: Each constructed Foundation object conforms to CKRecordValue and
    // the generated binding copies it into the record.
    unsafe { record.setObject_forKey(Some(&value), &key) };
}

fn create_asset_staging_directory() -> Result<PathBuf, CloudKitError> {
    let directory = std::env::temp_dir().join(format!("pasters-cloudkit-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).map_err(|_| CloudKitError::AssetDirectoryCreationFailed)?;
    if set_mode(&directory, 0o700).is_err() {
        let _ = fs::remove_dir(&directory);
        return Err(CloudKitError::AssetDirectoryCreationFailed);
    }
    Ok(directory)
}

fn write_protected(path: &Path, bytes: &[u8]) -> Result<(), CloudKitError> {
    fs::write(path, bytes).map_err(|_| CloudKitError::AssetStagingFailed)?;
    set_mode(path, 0o600).map_err(|_| CloudKitError::AssetStagingFailed)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

fn remaining(deadline: Instant, timeout_error: CloudKitError) -> Result<Duration, CloudKitError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(timeout_error)
}

fn map_account_status(status: CKAccountStatus) -> CloudKitAccountStatus {
    match status {
        CKAccountStatus::Available => CloudKitAccountStatus::Available,
        CKAccountStatus::Restricted => CloudKitAccountStatus::Restricted,
        CKAccountStatus::NoAccount => CloudKitAccountStatus::NoAccount,
        CKAccountStatus::TemporarilyUnavailable => CloudKitAccountStatus::TemporarilyUnavailable,
        _ => CloudKitAccountStatus::CouldNotDetermine,
    }
}
