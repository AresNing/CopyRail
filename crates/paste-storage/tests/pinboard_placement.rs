use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, DeviceId, DeviceMetadata,
    PinboardId, SearchFilters, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::{PinboardSharePermission, SqliteStore, StorageError};
use paste_sync::{SyncChangeKind, SyncEntityKind};

fn clip(store: &SqliteStore, text: &str) -> ClipId {
    store
        .insert_capture(&CapturedItem {
            captured_at: Utc::now(),
            source: SourceApplication::unknown(),
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Synthetic Mac".into(),
            },
            flags: CaptureFlags::default(),
            representations: vec![CapturedRepresentation::plain_text(text)],
        })
        .expect("synthetic fixture operation")
        .id
}

fn order(store: &SqliteStore, board: PinboardId) -> Vec<ClipId> {
    let query = SearchQuery {
        filters: SearchFilters {
            pinboard_ids: vec![board],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut all = Vec::new();
    loop {
        let page = store
            .search(&query, SearchPage::new(200, all.len() as u32))
            .expect("synthetic fixture operation");
        if page.is_empty() {
            break;
        }
        all.extend(page.into_iter().map(|hit| hit.item.id));
    }
    all
}

fn board(store: &SqliteStore, name: &str) -> PinboardId {
    store
        .create_pinboard(name, "#34c759")
        .expect("synthetic fixture operation")
        .id
}

fn ack(store: &SqliteStore) {
    loop {
        let batch = store
            .prepare_private_sync_batch(250)
            .expect("synthetic fixture operation");
        if batch.operations.is_empty() {
            break;
        }
        store
            .acknowledge_sync_changes(
                &batch
                    .operations
                    .iter()
                    .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
                    .collect::<Vec<_>>(),
            )
            .expect("synthetic fixture operation");
    }
}

#[test]
fn noncontiguous_selection_can_move_before_after_and_to_the_end() {
    let store = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let b = board(&store, "Order");
    let ids = (0..5)
        .map(|index| clip(&store, &format!("item {index}")))
        .collect::<Vec<_>>();
    store
        .pin_clips(b, &ids)
        .expect("synthetic fixture operation");
    assert!(
        store
            .place_pinboard_clips(b, &[ids[1], ids[3]], Some(ids[0]), false)
            .expect("synthetic fixture operation")
    );
    assert_eq!(
        order(&store, b),
        vec![ids[1], ids[3], ids[0], ids[2], ids[4]]
    );
    assert!(
        store
            .place_pinboard_clips(b, &[ids[1], ids[3]], Some(ids[2]), true)
            .expect("synthetic fixture operation")
    );
    assert_eq!(
        order(&store, b),
        vec![ids[0], ids[2], ids[1], ids[3], ids[4]]
    );
    assert!(
        store
            .place_pinboard_clips(b, &[ids[0]], None, true)
            .expect("synthetic fixture operation")
    );
    assert_eq!(
        order(&store, b),
        vec![ids[2], ids[1], ids[3], ids[4], ids[0]]
    );
    ack(&store);
    assert!(
        !store
            .place_pinboard_clips(b, &[ids[0]], None, true)
            .expect("synthetic fixture operation")
    );
    assert!(
        !store
            .place_pinboard_clips(b, &[ids[1], ids[3]], Some(ids[1]), false)
            .expect("synthetic fixture operation")
    );
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("synthetic fixture operation"),
        0
    );
}

#[test]
fn moving_between_boards_keeps_history_and_queues_both_membership_changes() {
    let store = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let a = board(&store, "Source");
    let b = board(&store, "Destination");
    let x = clip(&store, "Move me");
    let y = clip(&store, "Anchor");
    store.pin_clip(a, x).expect("synthetic fixture operation");
    store.pin_clip(b, y).expect("synthetic fixture operation");
    ack(&store);
    assert!(
        store
            .place_pinboard_clips(b, &[x], Some(y), false)
            .expect("synthetic fixture operation")
    );
    assert_eq!(order(&store, a), vec![]);
    assert_eq!(order(&store, b), vec![x, y]);
    assert!(
        store
            .get_clip(x)
            .expect("synthetic fixture operation")
            .is_some()
    );
    let pending = store
        .prepare_private_sync_batch(250)
        .expect("synthetic fixture operation");
    assert!(
        pending
            .operations
            .iter()
            .any(|op| op.envelope.change.entity.id == format!("{a}:{x}")
                && op.envelope.change.change == SyncChangeKind::Delete)
    );
    assert!(
        pending
            .operations
            .iter()
            .any(|op| op.envelope.change.entity.id == format!("{b}:{x}")
                && op.envelope.change.change == SyncChangeKind::Save)
    );
    assert!(
        pending
            .operations
            .iter()
            .all(|op| op.envelope.change.entity.kind == SyncEntityKind::PinboardMembership)
    );
    store.pin_clip(a, x).expect("synthetic fixture operation"); // menu/API pinning has the same move semantics
    assert_eq!(order(&store, b), vec![y]);
    assert_eq!(order(&store, a), vec![x]);
}

#[test]
fn malformed_or_missing_items_and_foreign_anchors_do_not_partially_move() {
    let store = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let a = board(&store, "Source");
    let b = board(&store, "Target");
    let x = clip(&store, "Source clip");
    let y = clip(&store, "Target clip");
    store.pin_clip(a, x).expect("synthetic fixture operation");
    store.pin_clip(b, y).expect("synthetic fixture operation");
    ack(&store);
    for (ids, anchor) in [
        (vec![], None),
        (vec![x, x], None),
        (vec![x, ClipId::new()], None),
        (vec![y, ClipId::new()], Some(y)),
        (vec![x], Some(x)),
        (vec![x], Some(ClipId::new())),
        (vec![x; 201], None),
    ] {
        assert!(store.place_pinboard_clips(b, &ids, anchor, false).is_err());
        assert_eq!(order(&store, a), vec![x]);
        assert_eq!(order(&store, b), vec![y]);
        assert_eq!(
            store
                .pending_sync_change_count()
                .expect("synthetic fixture operation"),
            0
        );
    }
}

fn share(store: &SqliteStore, id: PinboardId, permission: PinboardSharePermission) {
    store
        .register_accepted_pinboard_share(
            id,
            "Shared",
            "#34c759",
            &format!("PasteShare_{id}"),
            "test_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
            permission,
        )
        .expect("synthetic fixture operation");
}

#[test]
fn readonly_source_and_target_roll_back_the_whole_selection() {
    let store = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let shared = PinboardId::new();
    share(&store, shared, PinboardSharePermission::ReadWrite);
    let a = board(&store, "Local source");
    let b = board(&store, "Local target");
    let x = clip(&store, "Local clip");
    let y = clip(&store, "Shared clip");
    store.pin_clip(a, x).expect("synthetic fixture operation");
    store
        .pin_clip(shared, y)
        .expect("synthetic fixture operation");
    share(&store, shared, PinboardSharePermission::ReadOnly);
    ack(&store);
    assert!(matches!(
        store.place_pinboard_clips(b, &[x, y], None, true),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        store.place_pinboard_clips(shared, &[x], None, true),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(order(&store, a), vec![x]);
    assert_eq!(order(&store, shared), vec![y]);
    assert_eq!(order(&store, b), vec![]);
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("synthetic fixture operation"),
        0
    );
}

#[test]
fn reorder_preserves_unseen_members_and_survives_restart() {
    let directory = tempfile::tempdir().expect("synthetic fixture operation");
    let path = directory.path().join("placement.db");
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    let b = board(&store, "Large board");
    let ids = (0..205)
        .map(|index| clip(&store, &format!("member {index}")))
        .collect::<Vec<_>>();
    store
        .pin_clips(b, &ids[..200])
        .expect("synthetic fixture operation");
    store
        .pin_clips(b, &ids[200..])
        .expect("synthetic fixture operation");
    store
        .place_pinboard_clips(b, &[ids[3]], Some(ids[1]), false)
        .expect("synthetic fixture operation");
    let mut expected = ids.clone();
    expected.remove(3);
    expected.insert(1, ids[3]);
    assert_eq!(order(&store, b), expected);
    drop(store);
    let store = SqliteStore::open(path).expect("synthetic fixture operation");
    assert_eq!(order(&store, b), expected);
    assert_eq!(order(&store, b)[200..], ids[200..]);
}

#[test]
fn moved_selection_and_its_order_arrive_on_a_second_device() {
    let source = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let target = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let a = board(&source, "A");
    let b = board(&source, "B");
    let ids = (0..4)
        .map(|index| clip(&source, &format!("sync {index}")))
        .collect::<Vec<_>>();
    source
        .pin_clips(a, &ids[..2])
        .expect("synthetic fixture operation");
    source
        .pin_clips(b, &ids[2..])
        .expect("synthetic fixture operation");
    for moving in [false, true] {
        if moving {
            source
                .place_pinboard_clips(b, &ids[..2], Some(ids[3]), false)
                .expect("synthetic fixture operation");
        }
        let batch = source
            .prepare_private_sync_batch(250)
            .expect("synthetic fixture operation");
        let envelopes = batch
            .operations
            .iter()
            .map(|op| op.envelope.clone())
            .collect::<Vec<_>>();
        target
            .apply_remote_sync_batch(&envelopes, &batch.blobs)
            .expect("synthetic fixture operation");
        ack(&source);
        assert_eq!(order(&target, a), order(&source, a));
        assert_eq!(order(&target, b), order(&source, b));
    }
    assert_eq!(order(&target, b), vec![ids[2], ids[0], ids[1], ids[3]]);
    assert_eq!(
        target
            .pending_sync_change_count()
            .expect("synthetic fixture operation"),
        0
    );
}
