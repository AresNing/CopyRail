use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, DeviceId, DeviceMetadata,
    PinboardId, SourceApplication,
};
use paste_storage::{PinboardShare, PinboardSharePermission, SqliteStore, StorageError};
use paste_sync::{
    PreparedSyncBatch, SyncChangeKind, SyncEntityKind, SyncEnvelope, SyncPayload, SyncScope,
};

fn capture(text: &str) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now(),
        source: SourceApplication::unknown(),
        device: DeviceMetadata {
            id: DeviceId::new(),
            display_name: "Synthetic Mac".into(),
        },
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

fn fixture() -> (PreparedSyncBatch, PinboardId, ClipId) {
    let source = SqliteStore::open_in_memory().expect("source");
    let clip = source
        .insert_capture(&capture("shared synthetic body"))
        .expect("capture");
    let board = source
        .create_pinboard("Shared fixture", "#34c759")
        .expect("board");
    source.pin_clip(board.id, clip.id).expect("pin");
    source
        .reserve_owned_pinboard_share(board.id, PinboardSharePermission::ReadWrite)
        .expect("reserve");
    let share = source
        .activate_owned_pinboard_share(
            board.id,
            "synthetic_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
        )
        .expect("activate");
    (
        source
            .prepare_pinboard_share_sync_batch(share.id, 250)
            .expect("batch"),
        board.id,
        clip.id,
    )
}

fn accept(
    store: &SqliteStore,
    board: PinboardId,
    permission: PinboardSharePermission,
) -> PinboardShare {
    store
        .register_accepted_pinboard_share(
            board,
            "Invitation title",
            "#34c759",
            &format!("PasteShare_{board}"),
            "synthetic_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
            permission,
        )
        .expect("accept")
}

fn current(store: &SqliteStore, id: uuid::Uuid) -> PinboardShare {
    store
        .list_pinboard_shares()
        .expect("shares")
        .into_iter()
        .find(|share| share.id == id)
        .expect("registered share")
}

fn entity(batch: &PreparedSyncBatch, kind: SyncEntityKind) -> SyncEnvelope {
    batch
        .operations
        .iter()
        .find(|operation| operation.envelope.change.entity.kind == kind)
        .expect("source entity")
        .envelope
        .clone()
}

#[test]
fn shared_page_does_not_overwrite_private_history_even_when_remote_ids_collide() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let private = target
        .insert_capture(&capture("private synthetic body"))
        .expect("private");
    let before = target
        .prepare_private_sync_batch(250)
        .expect("private snapshot");
    let share = accept(&target, board, PinboardSharePermission::ReadOnly);
    let mut remote = entity(&batch, SyncEntityKind::Clip);
    remote.change.entity.id = private.id.to_string();
    let Some(SyncPayload::Clip(clip)) = &mut remote.payload else {
        panic!("clip")
    };
    clip.id = private.id;
    let report = target
        .stage_pinboard_share_sync_page(&share, &[remote.clone()], &batch.blobs, b"page-1")
        .expect("receive read-only share");
    assert_eq!(
        (report.received, report.duplicates, report.pending),
        (1, 0, 1)
    );
    assert_eq!(
        target
            .get_clip(private.id)
            .expect("private")
            .expect("exists"),
        private
    );
    assert_eq!(
        target
            .prepare_private_sync_batch(250)
            .expect("private unchanged"),
        before
    );
    assert_eq!(
        target
            .read_pinboard_share_inbox(share.id, 250)
            .expect("inbox")
            .envelopes,
        vec![remote]
    );
    assert_eq!(
        current(&target, share.id).server_change_token.as_deref(),
        Some(b"page-1".as_slice())
    );
    assert_eq!(target.list_pinboards().expect("board")[0].item_count, 0);
    assert!(
        target
            .load_sync_token("private")
            .expect("private token")
            .is_none()
    );
}

#[test]
fn invalid_later_record_or_blob_rolls_back_the_whole_page_and_checkpoint() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let share = accept(&target, board, PinboardSharePermission::ReadWrite);
    let good = entity(&batch, SyncEntityKind::Clip);
    let mut wrong_scope = good.clone();
    wrong_scope.scope = SyncScope::Private;
    let mut wrong_board = entity(&batch, SyncEntityKind::Pinboard);
    let other = PinboardId::new();
    wrong_board.change.entity.id = other.to_string();
    if let Some(SyncPayload::Pinboard(value)) = &mut wrong_board.payload {
        value.id = other;
    }
    let mut bad_membership = entity(&batch, SyncEntityKind::PinboardMembership);
    if let Some(SyncPayload::PinboardMembership(value)) = &mut bad_membership.payload {
        value.position = -1;
    }
    let mut bad_hash = good.clone();
    if let Some(SyncPayload::Clip(value)) = &mut bad_hash.payload {
        value.content_hash[0] ^= 1;
    }
    bad_hash.change.operation_id = uuid::Uuid::new_v4();
    for bad in [wrong_scope, wrong_board, bad_membership, bad_hash] {
        assert!(
            target
                .stage_pinboard_share_sync_page(
                    &share,
                    &[good.clone(), bad],
                    &batch.blobs,
                    b"must-not-commit"
                )
                .is_err()
        );
        assert_eq!(target.pending_shared_download_count().expect("count"), 0);
        assert!(current(&target, share.id).server_change_token.is_none());
    }
    let mut blobs = batch.blobs.clone();
    blobs[0].bytes[0] ^= 1;
    assert!(
        target
            .stage_pinboard_share_sync_page(&share, &[good], &blobs, b"bad-blob")
            .is_err()
    );
    assert_eq!(target.pending_shared_download_count().expect("count"), 0);
}

#[test]
fn replay_is_idempotent_but_reusing_an_operation_uuid_with_new_content_is_rejected() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let share = accept(&target, board, PinboardSharePermission::ReadWrite);
    let record = entity(&batch, SyncEntityKind::Clip);
    target
        .stage_pinboard_share_sync_page(
            &share,
            &[record.clone(), record.clone()],
            &batch.blobs,
            b"one",
        )
        .expect("duplicate in page");
    let share = current(&target, share.id);
    let replay = target
        .stage_pinboard_share_sync_page(&share, std::slice::from_ref(&record), &[], b"two")
        .expect("replay without reloading asset");
    assert_eq!(
        (replay.received, replay.duplicates, replay.pending),
        (0, 1, 1)
    );
    let mut tampered = record.clone();
    if let Some(SyncPayload::Clip(value)) = &mut tampered.payload {
        value.title = "different title".into();
    }
    let share = current(&target, share.id);
    assert!(matches!(
        target.stage_pinboard_share_sync_page(&share, &[tampered], &[], b"three"),
        Err(StorageError::SharedOperationMismatch)
    ));
    assert_eq!(
        current(&target, share.id).server_change_token,
        share.server_change_token
    );
    assert_eq!(
        target
            .read_pinboard_share_inbox(share.id, 250)
            .expect("read")
            .envelopes,
        vec![record]
    );
}

#[test]
fn attachment_hashes_cannot_read_private_blobs_or_another_shares_inbox() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    target
        .insert_capture(&capture("shared synthetic body"))
        .expect("same bytes are private");
    let first = accept(&target, board, PinboardSharePermission::ReadOnly);
    let record = entity(&batch, SyncEntityKind::Clip);
    assert!(
        target
            .stage_pinboard_share_sync_page(
                &first,
                std::slice::from_ref(&record),
                &[],
                b"private-hash-probe"
            )
            .is_err()
    );
    target
        .stage_pinboard_share_sync_page(&first, std::slice::from_ref(&record), &batch.blobs, b"one")
        .expect("explicit attachment");
    let second = accept(
        &target,
        PinboardId::new(),
        PinboardSharePermission::ReadOnly,
    );
    assert!(
        target
            .stage_pinboard_share_sync_page(
                &second,
                std::slice::from_ref(&record),
                &[],
                b"other-share-probe"
            )
            .is_err()
    );
    target
        .stage_pinboard_share_sync_page(
            &second,
            std::slice::from_ref(&record),
            &batch.blobs,
            b"two",
        )
        .expect("same operation UUID in a different share is independent");
    let a = target
        .read_pinboard_share_inbox(first.id, 250)
        .expect("first");
    let b = target
        .read_pinboard_share_inbox(second.id, 250)
        .expect("second");
    assert_eq!(a, b);
    let mut new_version = record;
    new_version.change.operation_id = uuid::Uuid::new_v4();
    target
        .stage_pinboard_share_sync_page(&current(&target, first.id), &[new_version], &[], b"three")
        .expect("reuse same-share attachment");
    assert_eq!(
        target.pending_shared_download_count().expect("both shares"),
        3
    );
}

#[test]
fn stale_checkpoints_identity_or_permissions_never_commit_or_reset_newer_progress() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let original = accept(&target, board, PinboardSharePermission::ReadOnly);
    target
        .stage_pinboard_share_sync_page(&original, &[], &[], b"newer")
        .expect("empty checkpoint");
    assert!(matches!(
        target.stage_pinboard_share_sync_page(&original, &[], &[], b"late"),
        Err(StorageError::StaleSharedDownload)
    ));
    assert!(matches!(
        target.reset_pinboard_share_download_checkpoint(&original),
        Err(StorageError::StaleSharedDownload)
    ));
    let current = current(&target, original.id);
    let mut wrong_owner = current.clone();
    wrong_owner.owner_name = Some("other-owner".into());
    let mut wrong_id = current.clone();
    wrong_id.id = uuid::Uuid::new_v4();
    let mut wrong_role = current.clone();
    wrong_role.role = paste_storage::PinboardShareRole::Owner;
    for snapshot in [wrong_owner, wrong_id, wrong_role] {
        assert!(matches!(
            target.stage_pinboard_share_sync_page(&snapshot, &[], &[], b"invalid"),
            Err(StorageError::StaleSharedDownload)
        ));
    }
    accept(&target, board, PinboardSharePermission::ReadWrite);
    assert!(matches!(
        target.stage_pinboard_share_sync_page(
            &current,
            &[entity(&batch, SyncEntityKind::Clip)],
            &batch.blobs,
            b"old-permission"
        ),
        Err(StorageError::StaleSharedDownload)
    ));
    assert_eq!(target.pending_shared_download_count().expect("count"), 0);
}

#[test]
fn cross_page_dependencies_and_tombstones_remain_lossless_across_restart_and_token_reset() {
    let (batch, board, clip_id) = fixture();
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("history.db");
    let target = SqliteStore::open(&path).expect("target");
    let share = accept(&target, board, PinboardSharePermission::ReadOnly);
    let mut records = vec![
        entity(&batch, SyncEntityKind::PinboardMembership),
        entity(&batch, SyncEntityKind::Clip),
        entity(&batch, SyncEntityKind::Pinboard),
    ];
    let mut deleted = records[0].clone();
    deleted.change.operation_id = uuid::Uuid::new_v4();
    deleted.change.change = SyncChangeKind::Delete;
    deleted.payload = None;
    records.push(deleted);
    for (index, record) in records.iter().enumerate() {
        target
            .stage_pinboard_share_sync_page(
                &current(&target, share.id),
                std::slice::from_ref(record),
                &batch.blobs,
                format!("page-{index}").as_bytes(),
            )
            .expect("out-of-order input");
    }
    let before = target
        .read_pinboard_share_inbox(share.id, 250)
        .expect("before restart");
    assert_eq!(before.envelopes.len(), 4);
    assert!(
        target
            .get_clip(clip_id)
            .expect("no premature projection")
            .is_none()
    );
    drop(target);
    let target = SqliteStore::open(&path).expect("restart");
    assert_eq!(
        target
            .read_pinboard_share_inbox(share.id, 250)
            .expect("reopen"),
        before
    );
    target
        .reset_pinboard_share_download_checkpoint(&current(&target, share.id))
        .expect("expire token");
    let replay = target
        .stage_pinboard_share_sync_page(&current(&target, share.id), &records, &[], b"replayed")
        .expect("replay");
    assert_eq!(
        (replay.received, replay.duplicates, replay.pending),
        (0, 4, 4)
    );
    assert_eq!(
        target.list_pinboards().expect("registered board")[0].name,
        "Invitation title"
    );
    assert_eq!(
        target.pending_sync_change_count().expect("no private echo"),
        0
    );
}

#[test]
fn inbox_is_included_in_backup_and_v15_migration_preserves_existing_queued_bytes() {
    let (batch, board, _) = fixture();
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("history.db");
    let target = SqliteStore::open(&path).expect("target");
    target
        .insert_capture(&capture("private before migration"))
        .expect("private");
    let private = target.prepare_private_sync_batch(250).expect("snapshot");
    let legacy_share = accept(&target, board, PinboardSharePermission::ReadOnly);
    target
        .stage_pinboard_share_sync_page(&legacy_share, &[], &[], b"legacy-token-without-journal")
        .expect("legacy token fixture");
    target
        .save_sync_token("private", b"private-checkpoint")
        .expect("private checkpoint");
    drop(target);
    let legacy = rusqlite::Connection::open(&path).expect("fixture schema");
    legacy.execute_batch("DROP TABLE cloudkit_share_inbox_blob_refs; DROP TABLE cloudkit_share_inbox_blobs; DROP TABLE cloudkit_share_inbox; PRAGMA user_version = 15;").expect("genuine v15 fixture");
    drop(legacy);
    let target = SqliteStore::open(&path).expect("migrate");
    assert!(
        current(&target, legacy_share.id)
            .server_change_token
            .is_none(),
        "old shared token must replay into new inbox"
    );
    assert_eq!(
        target
            .load_sync_token("private")
            .expect("private checkpoint"),
        Some(b"private-checkpoint".to_vec())
    );
    assert_eq!(
        target
            .prepare_private_sync_batch(250)
            .expect("unchanged private"),
        private
    );
    let share = accept(&target, board, PinboardSharePermission::ReadOnly);
    target
        .stage_pinboard_share_sync_page(
            &share,
            &[entity(&batch, SyncEntityKind::Clip)],
            &batch.blobs,
            b"checkpoint",
        )
        .expect("stage");
    let expected = target
        .read_pinboard_share_inbox(share.id, 250)
        .expect("inbox");
    let backup = directory.path().join("backup.pasters");
    target.export_backup(&backup).expect("backup");
    let restored = SqliteStore::open_in_memory().expect("restored");
    restored.restore_backup(&backup).expect("restore");
    assert_eq!(
        restored
            .read_pinboard_share_inbox(share.id, 250)
            .expect("restored inbox"),
        expected
    );
    assert_eq!(
        current(&restored, share.id).server_change_token.as_deref(),
        Some(b"checkpoint".as_slice())
    );
    assert_eq!(
        restored.prepare_private_sync_batch(250).expect("private"),
        private
    );
}

#[test]
fn bounded_reads_and_invalid_page_limits_do_not_drop_received_data() {
    let (batch, board, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let share = accept(&target, board, PinboardSharePermission::ReadOnly);
    let records = batch
        .operations
        .iter()
        .map(|operation| operation.envelope.clone())
        .collect::<Vec<_>>();
    assert!(
        target
            .stage_pinboard_share_sync_page(&share, &records, &batch.blobs, b"")
            .is_err()
    );
    assert!(
        target
            .stage_pinboard_share_sync_page(
                &share,
                &vec![records[0].clone(); 251],
                &batch.blobs,
                b"over-count"
            )
            .is_err()
    );
    let report = target
        .stage_pinboard_share_sync_page(&share, &records, &batch.blobs, b"valid")
        .expect("stage");
    assert_eq!(
        target
            .read_pinboard_share_inbox(share.id, 1)
            .expect("one")
            .envelopes
            .len(),
        1
    );
    assert!(target.read_pinboard_share_inbox(share.id, 0).is_err());
    assert!(
        target
            .read_pinboard_share_inbox(uuid::Uuid::new_v4(), 1)
            .is_err()
    );
    assert_eq!(
        target
            .pending_shared_download_count()
            .expect("read did not consume"),
        report.pending
    );
}
