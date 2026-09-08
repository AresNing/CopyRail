use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, DeviceId, DeviceMetadata,
    PinboardId, SourceApplication,
};
use paste_storage::{SqliteStore, StorageError};
use paste_sync::{PreparedSyncBatch, SyncEntityKind, SyncEnvelope, SyncPayload};

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

fn fixture() -> (SqliteStore, PreparedSyncBatch, PinboardId, ClipId) {
    let source = SqliteStore::open_in_memory().expect("source");
    let clip = source
        .insert_capture(&capture("page fixture"))
        .expect("capture");
    let board = source
        .create_pinboard("Page fixture", "#34c759")
        .expect("board");
    source.pin_clip(board.id, clip.id).expect("pin");
    let batch = source
        .prepare_private_sync_batch(250)
        .expect("source batch");
    let ids = batch
        .operations
        .iter()
        .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
        .collect::<Vec<_>>();
    source.acknowledge_sync_changes(&ids).expect("ack source");
    (source, batch, board.id, clip.id)
}

fn entity(batch: &PreparedSyncBatch, kind: SyncEntityKind) -> SyncEnvelope {
    batch
        .operations
        .iter()
        .find(|op| op.envelope.change.entity.kind == kind)
        .expect("entity")
        .envelope
        .clone()
}

#[test]
fn every_cross_page_dependency_order_converges_without_echo() {
    let (_, batch, _, _) = fixture();
    let records = [
        entity(&batch, SyncEntityKind::Clip),
        entity(&batch, SyncEntityKind::Pinboard),
        entity(&batch, SyncEntityKind::PinboardMembership),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let target = SqliteStore::open_in_memory().expect("target");
        for (page, index) in order.into_iter().enumerate() {
            let token = format!("synthetic-page-{page}").into_bytes();
            target
                .apply_private_sync_page(&[records[index].clone()], &batch.blobs, &token)
                .expect("apply out-of-order page");
            assert_eq!(
                target.load_sync_token("private").expect("checkpoint"),
                Some(token)
            );
        }
        assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 1);
        assert_eq!(
            target.pending_remote_membership_count().expect("deferred"),
            0
        );
        assert_eq!(target.pending_sync_change_count().expect("no echo"), 0);
        let replay = target
            .apply_private_sync_page(&records, &batch.blobs, b"replayed")
            .expect("replay pages");
        assert_eq!(replay.applied, 0);
        assert_eq!(replay.ignored, 3);
    }
}

#[test]
fn waiting_membership_and_checkpoint_survive_restart() {
    let (_, batch, _, _) = fixture();
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("pages.db");
    let target = SqliteStore::open(&path).expect("target");
    let report = target
        .apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::PinboardMembership)],
            &[],
            b"relation-page",
        )
        .expect("defer relation");
    assert_eq!(report.applied, 0);
    assert_eq!(report.deferred, 1);
    drop(target);
    let target = SqliteStore::open(&path).expect("reopen");
    assert_eq!(
        target.load_sync_token("private").expect("checkpoint"),
        Some(b"relation-page".to_vec())
    );
    assert_eq!(
        target
            .pending_remote_membership_count()
            .expect("durable pending relation"),
        1
    );
    target
        .apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::Clip)],
            &batch.blobs,
            b"clip-page",
        )
        .expect("clip page");
    let final_page = target
        .apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::Pinboard)],
            &[],
            b"board-page",
        )
        .expect("board page");
    assert_eq!(final_page.applied, 2);
    assert_eq!(final_page.deferred, 0);
    assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 1);
}

#[test]
fn rejected_page_rolls_back_entities_versions_deferred_relations_and_token() {
    let (_, batch, _, _) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    target
        .save_sync_token("private", b"previous-page")
        .expect("old checkpoint");
    let mut bad_board = entity(&batch, SyncEntityKind::Pinboard);
    if let Some(SyncPayload::Pinboard(board)) = &mut bad_board.payload {
        board.name.clear();
    }
    assert!(matches!(
        target.apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::Clip), bad_board],
            &batch.blobs,
            b"must-not-advance"
        ),
        Err(StorageError::InvalidPinboardName)
    ));
    assert!(
        target
            .list_history(Default::default())
            .expect("no partial clip")
            .is_empty()
    );
    assert_eq!(
        target
            .load_sync_token("private")
            .expect("old checkpoint preserved"),
        Some(b"previous-page".to_vec())
    );
    let mut bad_relation = entity(&batch, SyncEntityKind::PinboardMembership);
    if let Some(SyncPayload::PinboardMembership(relation)) = &mut bad_relation.payload {
        relation.position = -1;
    }
    assert!(matches!(
        target.apply_private_sync_page(&[bad_relation], &[], b"bad-relation"),
        Err(StorageError::InvalidRemoteSyncBatch)
    ));
    assert_eq!(
        target
            .pending_remote_membership_count()
            .expect("not persisted"),
        0
    );
    assert!(matches!(
        target.apply_private_sync_page(&[], &[], b""),
        Err(StorageError::InvalidSyncToken)
    ));
    target
        .apply_private_sync_page(
            &batch
                .operations
                .iter()
                .map(|op| op.envelope.clone())
                .collect::<Vec<_>>(),
            &batch.blobs,
            b"valid-retry",
        )
        .expect("valid records can retry after rollback");
    assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 1);
}

#[test]
fn later_unpin_supersedes_a_waiting_relation_and_replay_cannot_resurrect_it() {
    let (source, batch, board, clip) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    let relation = entity(&batch, SyncEntityKind::PinboardMembership);
    target
        .apply_private_sync_page(std::slice::from_ref(&relation), &[], b"waiting")
        .expect("defer");
    source.unpin_clip(board, clip).expect("unpin source");
    let deletion = source.prepare_private_sync_batch(250).expect("deletion");
    target
        .apply_private_sync_page(
            &[entity(&deletion, SyncEntityKind::PinboardMembership)],
            &[],
            b"removed",
        )
        .expect("remove deferred relation");
    assert_eq!(
        target
            .pending_remote_membership_count()
            .expect("deferred removed"),
        0
    );
    target
        .apply_private_sync_page(
            &[
                entity(&batch, SyncEntityKind::Clip),
                entity(&batch, SyncEntityKind::Pinboard),
            ],
            &batch.blobs,
            b"parents",
        )
        .expect("parents arrive");
    let replay = target
        .apply_private_sync_page(&[relation], &[], b"stale-replay")
        .expect("old save ignored");
    assert_eq!(replay.ignored, 1);
    assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 0);
}

#[test]
fn a_deleted_parent_defers_membership_until_the_clip_is_restored() {
    let (source, batch, _, clip) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    target
        .apply_private_sync_page(
            &[
                entity(&batch, SyncEntityKind::Clip),
                entity(&batch, SyncEntityKind::Pinboard),
            ],
            &batch.blobs,
            b"parents",
        )
        .expect("parents");
    source.delete_clip(clip, Utc::now()).expect("delete source");
    let deleted = source
        .prepare_private_sync_batch(250)
        .expect("delete batch");
    target
        .apply_private_sync_page(&[entity(&deleted, SyncEntityKind::Clip)], &[], b"deleted")
        .expect("remote deletion");
    let deferred = target
        .apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::PinboardMembership)],
            &[],
            b"relation",
        )
        .expect("defer for deleted clip");
    assert_eq!(deferred.deferred, 1);
    source.undo_last_delete().expect("restore source");
    let restored = source
        .prepare_private_sync_batch(250)
        .expect("restore batch");
    let report = target
        .apply_private_sync_page(
            &[entity(&restored, SyncEntityKind::Clip)],
            &restored.blobs,
            b"restored",
        )
        .expect("restore drains relation");
    assert_eq!(report.deferred, 0);
    assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 1);
}

#[test]
fn large_ready_backlog_drains_in_bounded_batches_including_empty_cloud_pages() {
    let source = SqliteStore::open_in_memory().expect("source");
    let board = source
        .create_pinboard("Large synthetic board", "#34c759")
        .expect("board");
    for index in 0..251 {
        let clip = source
            .insert_capture(&capture(&format!("fixture {index}")))
            .expect("clip");
        source.pin_clip(board.id, clip.id).expect("pin");
    }
    let mut envelopes = Vec::new();
    let mut blobs = Vec::new();
    loop {
        let batch = source.prepare_private_sync_batch(250).expect("source page");
        if batch.operations.is_empty() {
            break;
        }
        source
            .acknowledge_sync_changes(
                &batch
                    .operations
                    .iter()
                    .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
                    .collect::<Vec<_>>(),
            )
            .expect("ack");
        envelopes.extend(batch.operations.into_iter().map(|op| op.envelope));
        blobs.extend(batch.blobs);
    }
    let target = SqliteStore::open_in_memory().expect("target");
    for kind in [SyncEntityKind::PinboardMembership, SyncEntityKind::Clip] {
        let records = envelopes
            .iter()
            .filter(|envelope| envelope.change.entity.kind == kind)
            .cloned()
            .collect::<Vec<_>>();
        for chunk in records.chunks(250) {
            target
                .apply_private_sync_page(chunk, &blobs, b"synthetic-page")
                .expect("out-of-order page");
        }
    }
    assert_eq!(
        target
            .pending_remote_membership_count()
            .expect("all waiting for board"),
        251
    );
    let board_record = envelopes
        .into_iter()
        .find(|envelope| envelope.change.entity.kind == SyncEntityKind::Pinboard)
        .expect("board");
    let report = target
        .apply_private_sync_page(&[board_record], &[], b"board")
        .expect("board drains bounded relations");
    assert_eq!(report.applied, 251);
    assert_eq!(report.deferred, 1);
    let final_report = target
        .apply_private_sync_page(&[], &[], b"empty-next-page")
        .expect("empty page still drains");
    assert_eq!(final_report.applied, 1);
    assert_eq!(final_report.deferred, 0);
    assert_eq!(target.list_pinboards().expect("board")[0].item_count, 251);
}

#[test]
fn deferred_pin_protects_its_clip_from_retention_before_the_board_arrives() {
    let (_, batch, _, clip) = fixture();
    let target = SqliteStore::open_in_memory().expect("target");
    target
        .apply_private_sync_page(
            &[entity(&batch, SyncEntityKind::PinboardMembership)],
            &[],
            b"relation",
        )
        .expect("defer relation");
    let mut old_clip = entity(&batch, SyncEntityKind::Clip);
    if let Some(SyncPayload::Clip(clip)) = &mut old_clip.payload {
        clip.captured_at -= chrono::Duration::days(30);
        clip.last_copied_at -= chrono::Duration::days(30);
    }
    target
        .apply_private_sync_page(&[old_clip], &batch.blobs, b"clip")
        .expect("old pinned clip arrives");
    let mut older_unpinned = capture("older unpinned");
    older_unpinned.captured_at -= chrono::Duration::seconds(1);
    target
        .insert_capture(&older_unpinned)
        .expect("unpinned fixture");
    target
        .insert_capture(&capture("newer unpinned"))
        .expect("unpinned fixture");
    let removed = target
        .apply_retention(
            paste_domain::RetentionPolicy {
                max_age_days: Some(1),
                max_unpinned_items: Some(1),
            },
            Utc::now(),
        )
        .expect("retention while dependencies are pending");
    assert_eq!(removed, 1, "only the older unpinned item is removed");
    assert!(
        target
            .get_clip(clip)
            .expect("protected pending pin")
            .is_some()
    );
    target
        .apply_private_sync_page(&[entity(&batch, SyncEntityKind::Pinboard)], &[], b"board")
        .expect("board arrives");
    assert_eq!(target.list_pinboards().expect("boards")[0].item_count, 1);
    assert_eq!(
        target.pending_remote_membership_count().expect("resolved"),
        0
    );
}
