use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ContentKind, DeviceId, DeviceMetadata,
    PinboardId, SourceApplication,
};
use paste_storage::{PinboardShare, PinboardSharePermission, SqliteStore, StorageError};
use paste_sync::{
    PreparedSyncBatch, SyncChangeKind, SyncEntityKind, SyncPayload, SyncScope, VersionOrder,
};

fn capture(text: &str) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now(),
        source: SourceApplication::unknown(),
        device: DeviceMetadata {
            id: DeviceId::new(),
            display_name: "Synthetic test Mac".into(),
        },
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

fn ack_ids(batch: &PreparedSyncBatch) -> Vec<uuid::Uuid> {
    batch
        .operations
        .iter()
        .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
        .collect()
}

fn acknowledge_private(store: &SqliteStore) {
    let batch = store
        .prepare_private_sync_batch(250)
        .expect("private batch");
    store
        .acknowledge_sync_changes(&ack_ids(&batch))
        .expect("ack private");
}

fn activate(store: &SqliteStore, board_id: PinboardId) -> PinboardShare {
    store
        .reserve_owned_pinboard_share(board_id, PinboardSharePermission::ReadWrite)
        .expect("reserve share");
    store
        .activate_owned_pinboard_share(
            board_id,
            "test_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
        )
        .expect("activate share")
}

fn accept(
    store: &SqliteStore,
    board_id: PinboardId,
    permission: PinboardSharePermission,
) -> Result<PinboardShare, StorageError> {
    store.register_accepted_pinboard_share(
        board_id,
        "Synthetic board",
        "#34c759",
        &format!("PasteShare_{board_id}"),
        "test_owner",
        "zone_share",
        "https://www.icloud.com/share/synthetic",
        permission,
    )
}

#[test]
fn pending_uuid_and_bytes_remain_immutable_after_edit_delete_and_restart() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("history.db");
    let store = SqliteStore::open(&path).expect("open");
    let clip = store
        .insert_capture(&capture("first revision"))
        .expect("capture");
    let first = store.prepare_private_sync_batch(1).expect("first revision");
    store
        .update_textual_clip(clip.id, ContentKind::Text, "second", "second revision")
        .expect("edit");
    assert_eq!(
        first,
        store
            .prepare_private_sync_batch(1)
            .expect("retry after edit")
    );
    store.delete_clip(clip.id, Utc::now()).expect("delete");
    drop(store);
    let store = SqliteStore::open(&path).expect("reopen");
    assert_eq!(
        first,
        store
            .prepare_private_sync_batch(1)
            .expect("retry after restart")
    );
    assert_eq!(first.blobs[0].bytes, b"first revision");
    store
        .acknowledge_sync_changes(&ack_ids(&first))
        .expect("ack first");
    let second = store
        .prepare_private_sync_batch(1)
        .expect("second revision snapshot");
    assert_eq!(second.blobs[0].bytes, b"second revision");
    assert_eq!(
        first.operations[0]
            .envelope
            .change
            .version
            .compare(&second.operations[0].envelope.change.version),
        VersionOrder::Before
    );
    store
        .acknowledge_sync_changes(&ack_ids(&second))
        .expect("ack second");
    let deleted = store.prepare_private_sync_batch(1).expect("tombstone");
    assert_eq!(
        deleted.operations[0].envelope.change.change,
        SyncChangeKind::Delete
    );
    assert!(deleted.blobs.is_empty());
    store
        .acknowledge_sync_changes(&ack_ids(&deleted))
        .expect("ack deletion");
    assert_eq!(store.pending_sync_change_count().expect("count"), 0);
}

#[test]
fn shared_attachment_lives_until_both_deliveries_acknowledge_and_ack_is_atomic() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("history.db");
    let store = SqliteStore::open(&path).expect("open");
    let clip = store
        .insert_capture(&capture("shared original"))
        .expect("capture");
    let board = store
        .create_pinboard("Synthetic", "#34c759")
        .expect("board");
    store.pin_clip(board.id, clip.id).expect("pin");
    let share = activate(&store, board.id);
    let initial = store
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("initial share batch");
    assert_eq!(initial.operations.len(), 3);
    let counts = store
        .pending_pinboard_share_change_count(share.id)
        .expect("count");
    assert_eq!(activate(&store, board.id).id, share.id);
    assert_eq!(
        counts,
        store
            .pending_pinboard_share_change_count(share.id)
            .expect("idempotent activate")
    );
    store
        .update_textual_clip(clip.id, ContentKind::Text, "edited", "shared replacement")
        .expect("edit");
    acknowledge_private(&store);
    assert_eq!(store.blob_count().expect("old bytes retained for share"), 2);
    let ids = ack_ids(&initial);
    let before = store
        .pending_pinboard_share_change_count(share.id)
        .expect("count");
    assert!(matches!(
        store.acknowledge_pinboard_share_changes(share.id, &[ids[0], uuid::Uuid::new_v4()]),
        Err(StorageError::UnknownSharedSyncOperation)
    ));
    assert_eq!(
        before,
        store
            .pending_pinboard_share_change_count(share.id)
            .expect("rollback")
    );
    store
        .acknowledge_pinboard_share_changes(share.id, &ids)
        .expect("shared ack");
    store
        .acknowledge_pinboard_share_changes(share.id, &ids)
        .expect("idempotent shared retry");
    assert_eq!(store.blob_count().expect("old bytes released"), 1);
    let latest = store
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("latest shared edit");
    assert_eq!(latest.blobs[0].bytes, b"shared replacement");
    store
        .acknowledge_pinboard_share_changes(share.id, &ack_ids(&latest))
        .expect("final shared ack");
    let check = rusqlite::Connection::open(&path).expect("inspect synthetic db");
    let snapshots: u32 = check
        .query_row("SELECT COUNT(*) FROM sync_outbox_snapshots", [], |row| {
            row.get(0)
        })
        .expect("snapshots");
    assert_eq!(snapshots, 0);
    assert!(matches!(
        store.delete_pinboard(board.id),
        Err(StorageError::PinboardAlreadyShared)
    ));
}

#[test]
fn newly_shared_board_includes_remote_imported_and_previously_acknowledged_clips() {
    let source = SqliteStore::open_in_memory().expect("source");
    let imported = source
        .insert_capture(&capture("imported content"))
        .expect("source capture");
    let batch = source
        .prepare_private_sync_batch(250)
        .expect("source batch");
    let store = SqliteStore::open_in_memory().expect("target");
    store
        .apply_remote_sync_batch(
            &batch
                .operations
                .iter()
                .map(|op| op.envelope.clone())
                .collect::<Vec<_>>(),
            &batch.blobs,
        )
        .expect("private import");
    let local = store
        .insert_capture(&capture("previously acknowledged content"))
        .expect("local");
    let board = store
        .create_pinboard("Imported board", "#34c759")
        .expect("board");
    store
        .pin_clips(board.id, &[imported.id, local.id])
        .expect("pin");
    acknowledge_private(&store);
    let share = activate(&store, board.id);
    let shared = store
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("shared snapshot");
    assert_eq!(shared.operations.len(), 5);
    assert_eq!(shared.blobs.len(), 2);
    assert!(
        shared
            .operations
            .iter()
            .all(|op| op.envelope.scope == SyncScope::Shared)
    );
    let shared_ids = shared
        .operations
        .iter()
        .filter_map(|op| match &op.envelope.payload {
            Some(SyncPayload::Clip(clip)) => Some(clip.id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(shared_ids.contains(&imported.id) && shared_ids.contains(&local.id));
}

#[test]
fn private_import_rejects_shared_scope_without_partial_writes() {
    let source = SqliteStore::open_in_memory().expect("source");
    source
        .insert_capture(&capture("private fixture"))
        .expect("capture");
    let batch = source.prepare_private_sync_batch(250).expect("batch");
    let mut shared = batch.operations[0].envelope.clone();
    shared.scope = SyncScope::Shared;
    let target = SqliteStore::open_in_memory().expect("target");
    assert!(matches!(
        target.apply_remote_sync_batch(
            &[batch.operations[0].envelope.clone(), shared],
            &batch.blobs
        ),
        Err(StorageError::InvalidRemoteSyncBatch)
    ));
    assert!(
        target
            .list_history(Default::default())
            .expect("history")
            .is_empty()
    );
}

#[test]
fn readonly_share_blocks_indirect_mutations_and_preserves_recopy_metadata() {
    let store = SqliteStore::open_in_memory().expect("store");
    let board = PinboardId::new();
    accept(&store, board, PinboardSharePermission::ReadWrite).expect("join writable");
    let fixture = capture("readonly text");
    let clip = store.insert_capture(&fixture).expect("capture");
    let mut image_fixture = capture("");
    image_fixture.representations = vec![CapturedRepresentation {
        native_type: None,
        kind: paste_domain::RepresentationKind::Png,
        mime_type: Some("image/png".into()),
        file_name: None,
        bytes: b"\x89PNG\r\n\x1a\nsynthetic-placeholder".to_vec(),
    }];
    let image = store.insert_capture(&image_fixture).expect("image fixture");
    store
        .pin_clips(board, &[clip.id, image.id])
        .expect("pin writable");
    accept(&store, board, PinboardSharePermission::ReadOnly).expect("downgrade");
    let pending = store.pending_sync_change_count().expect("count");
    assert!(matches!(
        store.rename_clip(clip.id, "forbidden"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        store.update_textual_clip(clip.id, ContentKind::Text, "forbidden", "new text"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        store.update_image_content(image.id, b"\x89PNG\r\n\x1a\nreplacement"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        store.update_ocr_text(image.id, "forbidden ocr"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    let unshared = store
        .insert_capture(&capture("unshared fixture"))
        .expect("unshared");
    assert!(matches!(
        store.delete_clips(&[unshared.id, clip.id], Utc::now()),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(
        store
            .get_clip(unshared.id)
            .expect("batch rolled back")
            .is_some()
    );
    assert_eq!(
        store.get_clip(clip.id).expect("unchanged").expect("clip"),
        clip
    );
    assert_eq!(
        store
            .get_clip(image.id)
            .expect("unchanged image")
            .expect("clip"),
        image
    );
    let mut recopied = fixture;
    recopied.source.display_name = "Different app".into();
    let recopy = store.insert_capture(&recopied).expect("recopy allowed");
    assert_eq!(recopy, clip);
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("no forbidden outbox writes"),
        pending + 1
    );
}

#[test]
fn invitation_cannot_claim_a_local_board_or_replace_an_existing_share_owner() {
    let store = SqliteStore::open_in_memory().expect("store");
    let local = store
        .create_pinboard("Private board", "#34c759")
        .expect("local board");
    assert!(matches!(
        accept(&store, local.id, PinboardSharePermission::ReadOnly),
        Err(StorageError::PinboardAlreadyShared)
    ));
    let remote = PinboardId::new();
    let share = accept(&store, remote, PinboardSharePermission::ReadOnly).expect("accept");
    store
        .stage_pinboard_share_sync_page(&share, &[], &[], b"opaque test token")
        .expect("token");
    assert!(matches!(
        store.register_accepted_pinboard_share(
            remote,
            "Spoofed",
            "#34c759",
            &share.zone_name,
            "different_owner",
            "zone_share",
            "https://www.icloud.com/share/other",
            PinboardSharePermission::ReadWrite
        ),
        Err(StorageError::PinboardAlreadyShared)
    ));
    let retry =
        accept(&store, remote, PinboardSharePermission::ReadOnly).expect("same share retry");
    assert_eq!(retry.id, share.id);
    assert_eq!(
        retry.server_change_token,
        Some(b"opaque test token".to_vec())
    );
    assert!(
        !store
            .list_pinboards()
            .expect("boards")
            .iter()
            .find(|board| board.id == local.id)
            .expect("local")
            .is_shared
    );
}

#[test]
fn version_twelve_migration_renews_pending_ids_instead_of_guessing_old_payloads() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("legacy.db");
    let store = SqliteStore::open(&path).expect("open");
    let clip = store
        .insert_capture(&capture("old content"))
        .expect("capture");
    store
        .update_textual_clip(clip.id, ContentKind::Text, "latest", "latest content")
        .expect("edit");
    let gone = store
        .create_pinboard("Deleted board", "#34c759")
        .expect("board");
    store.delete_pinboard(gone.id).expect("delete");
    let old = store.pending_sync_changes(250).expect("old changes");
    drop(store);
    let legacy = rusqlite::Connection::open(&path).expect("synthetic downgrade");
    legacy.execute_batch("ALTER TABLE representations DROP COLUMN native_type; ALTER TABLE sync_outbox DROP COLUMN envelope_schema_version; DROP TABLE sync_outbox_blob_refs; DROP TABLE sync_outbox_snapshots; PRAGMA user_version = 12;").expect("v12 fixture");
    drop(legacy);
    let migrated = SqliteStore::open(&path).expect("migrate v12");
    let renewed = migrated
        .prepare_private_sync_batch(250)
        .expect("renewed snapshots");
    assert_eq!(renewed.operations.len(), 2);
    assert!(renewed.operations.iter().all(|op| {
        !old.iter()
            .any(|old| old.operation_id == op.envelope.change.operation_id)
    }));
    let clip_op = renewed
        .operations
        .iter()
        .find(|op| op.envelope.change.entity.kind == SyncEntityKind::Clip)
        .expect("renewed clip");
    assert!(
        matches!(&clip_op.envelope.payload, Some(SyncPayload::Clip(clip)) if clip.title == "latest")
    );
    assert_eq!(renewed.blobs[0].bytes, b"latest content");
    let tombstone = renewed
        .operations
        .iter()
        .find(|op| op.envelope.change.entity.kind == SyncEntityKind::Pinboard)
        .expect("renewed deletion");
    assert_eq!(tombstone.envelope.change.change, SyncChangeKind::Delete);
    drop(migrated);
    let reopened = SqliteStore::open(&path).expect("reopen upgraded database");
    assert_eq!(
        renewed,
        reopened
            .prepare_private_sync_batch(250)
            .expect("stable restart")
    );
}

#[test]
fn hard_retention_releases_obsolete_attachments_and_keeps_a_causal_tombstone() {
    let store = SqliteStore::open_in_memory().expect("store");
    let mut old = capture("expired history must not remain queued");
    old.captured_at -= chrono::Duration::days(30);
    let expired = store.insert_capture(&old).expect("old capture");
    let originally_prepared = store
        .prepare_private_sync_batch(1)
        .expect("old in-flight batch");
    let retained = store
        .insert_capture(&capture("retained history"))
        .expect("new capture");
    store
        .apply_retention(
            paste_domain::RetentionPolicy {
                max_age_days: Some(7),
                max_unpinned_items: None,
            },
            Utc::now(),
        )
        .expect("hard retention");
    assert!(store.get_clip(expired.id).expect("expired").is_none());
    assert!(store.get_clip(retained.id).expect("retained").is_some());
    assert_eq!(store.blob_count().expect("expired attachment released"), 1);
    let next = store
        .prepare_private_sync_batch(250)
        .expect("remaining queue");
    assert_eq!(next.blobs.len(), 1);
    assert_eq!(next.blobs[0].bytes, b"retained history");
    let tombstone = next
        .operations
        .iter()
        .find(|op| op.envelope.change.entity.id == expired.id.to_string())
        .expect("tombstone");
    assert_eq!(tombstone.envelope.change.change, SyncChangeKind::Delete);
    assert_eq!(
        originally_prepared.operations[0]
            .envelope
            .change
            .version
            .compare(&tombstone.envelope.change.version),
        VersionOrder::Before
    );
    store
        .acknowledge_sync_changes(&ack_ids(&originally_prepared))
        .expect("late network receipt remains idempotent");
    assert_eq!(
        next,
        store
            .prepare_private_sync_batch(250)
            .expect("late ack cannot lose newer tombstone")
    );
}
