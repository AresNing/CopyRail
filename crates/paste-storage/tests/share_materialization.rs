use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, DeviceId, DeviceMetadata,
    PinboardId, SearchPage, SourceApplication,
};
use paste_storage::{PinboardShare, PinboardSharePermission, SqliteStore, StorageError};
use paste_sync::{PreparedSyncBatch, SyncEntityKind, SyncEnvelope, SyncPayload};

#[path = "share_materialization/conflicts.rs"]
mod conflict_decisions;

fn capture(text: &str) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now(),
        source: SourceApplication::unknown(),
        device: DeviceMetadata {
            id: DeviceId::new(),
            display_name: "Synthetic peer".into(),
        },
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

fn owner() -> (SqliteStore, PinboardShare, ClipId) {
    let store = SqliteStore::open_in_memory().expect("isolated owner");
    let clip = store
        .insert_capture(&capture("shared original"))
        .expect("capture");
    let board = store.create_pinboard("Shared", "#34c759").expect("board");
    store.pin_clip(board.id, clip.id).expect("pin");
    store
        .reserve_owned_pinboard_share(board.id, PinboardSharePermission::ReadWrite)
        .expect("reserve");
    let share = store
        .activate_owned_pinboard_share(
            board.id,
            "fixture-owner",
            "fixture-share",
            "https://www.icloud.com/share/synthetic",
        )
        .expect("activate");
    (store, share, clip.id)
}

fn participant(
    board: PinboardId,
    permission: PinboardSharePermission,
) -> (SqliteStore, PinboardShare) {
    let store = SqliteStore::open_in_memory().expect("isolated participant");
    let share = store
        .register_accepted_pinboard_share(
            board,
            "Invitation",
            "#34c759",
            &format!("PasteShare_{board}"),
            "fixture-owner",
            "fixture-share",
            "https://www.icloud.com/share/synthetic",
            permission,
        )
        .expect("accept");
    (store, share)
}

fn current(store: &SqliteStore, share: &PinboardShare) -> PinboardShare {
    store
        .list_pinboard_shares()
        .expect("shares")
        .into_iter()
        .find(|s| s.id == share.id)
        .expect("share")
}

fn batch(store: &SqliteStore, share: &PinboardShare) -> PreparedSyncBatch {
    store
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("wire batch")
}

fn receive(store: &SqliteStore, share: &PinboardShare, batch: &PreparedSyncBatch) {
    let records = batch
        .operations
        .iter()
        .map(|op| op.envelope.clone())
        .collect::<Vec<_>>();
    store
        .stage_pinboard_share_sync_page(
            &current(store, share),
            &records,
            &batch.blobs,
            uuid::Uuid::new_v4().as_bytes(),
        )
        .expect("stage");
    store
        .materialize_pinboard_share(share.id, 250)
        .expect("project");
}

fn ack(store: &SqliteStore, share: &PinboardShare) {
    let ids = batch(store, share)
        .operations
        .iter()
        .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
        .collect::<Vec<_>>();
    store
        .acknowledge_pinboard_share_changes(share.id, &ids)
        .expect("ack");
}

fn imported(store: &SqliteStore) -> ClipId {
    store.list_history(SearchPage::default()).expect("history")[0].id
}

fn kind(batch: &PreparedSyncBatch, kind: SyncEntityKind) -> SyncEnvelope {
    batch
        .operations
        .iter()
        .find(|op| op.envelope.change.entity.kind == kind)
        .expect("entity")
        .envelope
        .clone()
}

#[test]
fn downloaded_shared_cards_are_visible_editable_and_write_back_the_original_wire_identity() {
    let (a, owner, original) = owner();
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    receive(&b, &guest, &batch(&a, &owner));
    let local = imported(&b);
    assert_ne!(local, original);
    assert_eq!(b.list_pinboards().expect("boards")[0].item_count, 1);
    assert_eq!(b.pending_shared_download_count().expect("drained"), 0);
    assert_eq!(b.pending_sync_change_count().expect("no private echo"), 0);
    b.rename_clip(local, "Participant edit")
        .expect("write access");
    assert_eq!(b.pending_sync_change_count().expect("shared only"), 0);
    let reply = batch(&b, &guest);
    let changed = kind(&reply, SyncEntityKind::Clip);
    assert_eq!(changed.change.entity.id, original.to_string());
    let Some(SyncPayload::Clip(item)) = changed.payload else {
        panic!("clip payload")
    };
    assert_eq!(item.id, original);
    assert_eq!(item.title, "Participant edit");
    receive(&a, &owner, &reply);
    assert_eq!(
        a.get_clip(original)
            .expect("owner original")
            .expect("exists")
            .title,
        "Participant edit"
    );
    let stable = batch(&b, &guest);
    b.rename_clip(local, "Later local edit")
        .expect("another edit");
    // Read the old operation after a newer snapshot exists; operation identity
    // and attachments must not be reconstructed from mutable current content.
    assert!(
        stable
            .operations
            .iter()
            .all(|op| op.envelope.change.entity.id == original.to_string())
    );
    assert_eq!(
        b.prepare_pinboard_share_sync_batch(guest.id, 1)
            .expect("same frozen first operation"),
        stable
    );
    ack(&b, &guest);
    assert!(batch(&b, &guest).operations.is_empty());
}

#[test]
fn content_and_membership_can_arrive_in_either_order_without_private_id_collisions() {
    let (a, owner, _) = owner();
    let initial = batch(&a, &owner);
    for member_first in [true, false] {
        let (b, guest) = participant(
            owner.pinboard_id.expect("board"),
            PinboardSharePermission::ReadWrite,
        );
        let private = b
            .insert_capture(&capture("private must survive"))
            .expect("private");
        let before = b.prepare_private_sync_batch(250).expect("private snapshot");
        let mut clip = kind(&initial, SyncEntityKind::Clip);
        clip.change.entity.id = private.id.to_string();
        if let Some(SyncPayload::Clip(item)) = &mut clip.payload {
            item.id = private.id;
        }
        let mut member = kind(&initial, SyncEntityKind::PinboardMembership);
        member.change.entity.id = format!("{}:{}", owner.pinboard_id.expect("board"), private.id);
        if let Some(SyncPayload::PinboardMembership(item)) = &mut member.payload {
            item.clip_id = private.id;
        }
        let records = if member_first {
            [member, clip]
        } else {
            [clip, member]
        };
        for (index, record) in records.into_iter().enumerate() {
            b.stage_pinboard_share_sync_page(
                &current(&b, &guest),
                &[record],
                &initial.blobs,
                &[index as u8 + 1],
            )
            .expect("page");
            b.materialize_pinboard_share(guest.id, 1)
                .expect("one record");
            assert_eq!(
                b.get_clip(private.id).expect("private").expect("exists"),
                private
            );
            assert_eq!(
                b.list_pinboards().expect("board")[0].item_count,
                index as u64
            );
        }
        assert_eq!(
            b.prepare_private_sync_batch(250)
                .expect("no private writes"),
            before
        );
        assert_eq!(
            b.list_history(SearchPage::default())
                .expect("two distinct cards")
                .len(),
            2
        );
    }
}

#[test]
fn readonly_imports_cannot_be_edited_moved_or_mutated_by_capture_deduplication() {
    let (a, owner, _) = owner();
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadOnly,
    );
    receive(&b, &guest, &batch(&a, &owner));
    let local = imported(&b);
    let before = b.get_clip(local).expect("card").expect("visible");
    assert!(matches!(
        b.rename_clip(local, "forbidden"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        b.unpin_clip(guest.pinboard_id.expect("board"), local),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    let private_board = b
        .create_pinboard("Private", "#34c759")
        .expect("private board");
    assert!(matches!(
        b.pin_clip(private_board.id, local),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(
        b.insert_capture(&capture("shared original"))
            .expect("recopy unchanged"),
        before
    );
    assert!(matches!(
        b.prepare_pinboard_share_sync_batch(guest.id, 250),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
}

#[test]
fn moved_owner_original_is_not_rewritten_and_readd_creates_an_independent_shared_copy() {
    let (a, owner, original) = owner();
    let board = owner.pinboard_id.expect("board");
    let (b, guest) = participant(board, PinboardSharePermission::ReadWrite);
    receive(&b, &guest, &batch(&a, &owner));
    ack(&a, &owner);
    let local = imported(&b);
    a.unpin_clip(board, original)
        .expect("owner removes original");
    a.rename_clip(original, "Private now")
        .expect("private edit");
    b.rename_clip(local, "Late collaborator update")
        .expect("concurrent shared edit");
    receive(&a, &owner, &batch(&b, &guest));
    assert_eq!(
        a.get_clip(original)
            .expect("private")
            .expect("exists")
            .title,
        "Private now"
    );
    assert_eq!(
        a.list_pinboards().expect("empty shared board")[0].item_count,
        0
    );
    // Deliver the owner's removal, then explicitly re-add its remote identity.
    let mut member = kind(&batch(&a, &owner), SyncEntityKind::PinboardMembership);
    member.change.operation_id = uuid::Uuid::new_v4();
    member.change.version.increment(DeviceId::new());
    member.change.change = paste_sync::SyncChangeKind::Save;
    member.payload = Some(SyncPayload::PinboardMembership(
        paste_sync::PinboardMembershipSnapshot {
            pinboard_id: board,
            clip_id: original,
            position: 0,
            created_at: Utc::now(),
        },
    ));
    a.stage_pinboard_share_sync_page(&current(&a, &owner), &[member], &[], b"readd")
        .expect("readd page");
    a.materialize_pinboard_share(owner.id, 250)
        .expect("fork binding");
    assert_eq!(
        a.get_clip(original)
            .expect("private")
            .expect("exists")
            .title,
        "Private now"
    );
    let cards = a
        .list_history(SearchPage::default())
        .expect("private and shared");
    assert_eq!(cards.len(), 2);
    assert!(
        cards
            .iter()
            .any(|c| c.id != original && c.title == "Late collaborator update")
    );
}

#[test]
fn compacted_receipts_reject_operation_mutation_and_private_transport_cannot_target_imports() {
    let (a, owner, _) = owner();
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    let initial = batch(&a, &owner);
    receive(&b, &guest, &initial);
    ack(&a, &owner);
    let original = kind(&initial, SyncEntityKind::Clip);
    a.rename_clip(
        ClipId::from_str(&original.change.entity.id).expect("ID"),
        "Newer",
    )
    .expect("new state");
    receive(&b, &guest, &batch(&a, &owner));
    let duplicate = b
        .stage_pinboard_share_sync_page(
            &current(&b, &guest),
            std::slice::from_ref(&original),
            &[],
            b"duplicate",
        )
        .expect("receipt, no blobs needed");
    assert_eq!(duplicate.duplicates, 1);
    let mut corrupt = original.clone();
    if let Some(SyncPayload::Clip(item)) = &mut corrupt.payload {
        item.title = "Reused UUID".into();
    }
    assert!(matches!(
        b.stage_pinboard_share_sync_page(&current(&b, &guest), &[corrupt], &initial.blobs, b"bad"),
        Err(StorageError::SharedOperationMismatch)
    ));
    let local = imported(&b);
    let mut private = original;
    private.scope = paste_sync::SyncScope::Private;
    private.change.entity.id = local.to_string();
    if let Some(SyncPayload::Clip(item)) = &mut private.payload {
        item.id = local;
    }
    assert!(matches!(
        b.apply_remote_sync_batch(&[private], &initial.blobs),
        Err(StorageError::InvalidRemoteSyncBatch)
    ));
}

use std::str::FromStr;

#[test]
fn raw_bytes_wire_snapshots_and_bindings_survive_restart_and_backup() {
    let (a, owner, original) = owner();
    let dir = tempfile::tempdir().expect("synthetic directory");
    let path = dir.path().join("guest.sqlite");
    let (memory, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    receive(&memory, &guest, &batch(&a, &owner));
    let local = imported(&memory);
    memory
        .update_textual_clip(
            local,
            paste_domain::ContentKind::Text,
            "edited",
            "first replacement bytes",
        )
        .expect("content edit");
    let frozen = batch(&memory, &guest);
    assert_eq!(frozen.blobs[0].bytes, b"first replacement bytes");
    memory.export_backup(&path).expect("synthetic backup");
    let reopened = SqliteStore::open(&path).expect("reopen");
    assert_eq!(imported(&reopened), local);
    reopened
        .update_textual_clip(
            local,
            paste_domain::ContentKind::Text,
            "later",
            "second replacement bytes",
        )
        .expect("later edit");
    assert_eq!(
        reopened
            .prepare_pinboard_share_sync_batch(guest.id, 1)
            .expect("frozen retry"),
        frozen
    );
    assert!(matches!(
        reopened.acknowledge_sync_changes(&[frozen.operations[0].envelope.change.operation_id]),
        Err(StorageError::UnknownSyncOperation)
    ));
    receive(&a, &owner, &batch(&reopened, &guest));
    assert_eq!(
        a.load_clipboard_payload(original).expect("updated bytes")[0].bytes,
        b"second replacement bytes"
    );
    // The private original fans out, but the participant's isolated copy does not.
    let private = a
        .prepare_private_sync_batch(250)
        .expect("owner private fanout");
    let item = kind(&private, SyncEntityKind::Clip);
    let Some(SyncPayload::Clip(item)) = item.payload else {
        panic!("clip")
    };
    assert_eq!(item.title, "later");
    assert_eq!(
        reopened
            .pending_sync_change_count()
            .expect("no private copy"),
        0
    );
}

#[test]
fn shared_delete_undo_and_unpin_do_not_leak_isolated_history_or_payloads() {
    let (a, owner, original) = owner();
    let board = owner.pinboard_id.expect("board");
    let (b, guest) = participant(board, PinboardSharePermission::ReadWrite);
    receive(&b, &guest, &batch(&a, &owner));
    ack(&a, &owner);
    let local = imported(&b);
    b.delete_clip(local, Utc::now()).expect("shared delete");
    receive(&a, &owner, &batch(&b, &guest));
    assert!(a.get_clip(original).expect("owner deleted").is_none());
    assert_eq!(a.list_pinboards().expect("no ghost count")[0].item_count, 0);
    ack(&b, &guest);
    b.undo_last_delete_batch().expect("shared undo");
    receive(&a, &owner, &batch(&b, &guest));
    assert!(a.get_clip(original).expect("restored original").is_some());
    ack(&b, &guest);
    let private_board = b.create_pinboard("Private", "#34c759").expect("board");
    assert!(matches!(
        b.pin_clip(private_board.id, local),
        Err(StorageError::SharedCrossScopeMove)
    ));
    b.unpin_clip(board, local).expect("shared removal");
    assert!(b.get_clip(local).expect("hidden isolated card").is_none());
    assert!(matches!(
        b.load_clipboard_payload(local),
        Err(StorageError::NotFound)
    ));
    receive(&a, &owner, &batch(&b, &guest));
    assert!(
        a.get_clip(original)
            .expect("owner private retained after unpin")
            .is_some()
    );
    assert_eq!(a.list_pinboards().expect("empty board")[0].item_count, 0);
}

#[test]
fn concurrent_shared_edits_converge_without_discarding_losing_content() {
    let (a, owner, original) = owner();
    let board = owner.pinboard_id.expect("board");
    let (b, guest) = participant(board, PinboardSharePermission::ReadWrite);
    let (c, observer) = participant(board, PinboardSharePermission::ReadWrite);
    receive(&b, &guest, &batch(&a, &owner));
    receive(&c, &observer, &batch(&a, &owner));
    ack(&a, &owner);
    let local = imported(&b);
    a.update_textual_clip(
        original,
        paste_domain::ContentKind::Text,
        "A revision",
        "A retained bytes",
    )
    .expect("A edit");
    b.update_textual_clip(
        local,
        paste_domain::ContentKind::Text,
        "B revision",
        "B retained bytes",
    )
    .expect("B edit");
    let first = batch(&a, &owner);
    let second = batch(&b, &guest);
    receive(&a, &owner, &second);
    receive(&b, &guest, &first);
    receive(&c, &observer, &second);
    receive(&c, &observer, &first);
    let winner = a.get_clip(original).expect("A").expect("visible").title;
    assert_eq!(
        b.get_clip(local).expect("B").expect("visible").title,
        winner
    );
    assert_eq!(
        c.get_clip(imported(&c)).expect("C").expect("visible").title,
        winner
    );
    assert_eq!(a.shared_conflict_count().expect("retained conflict"), 1);
    assert_eq!(b.shared_conflict_count().expect("retained conflict"), 1);
    let dir = tempfile::tempdir().expect("synthetic conflict backup");
    let path = dir.path().join("conflicts.sqlite");
    b.export_backup(&path).expect("backup");
    let conn = rusqlite::Connection::open(&path).expect("isolated inspection");
    let count: u32 = conn.query_row("SELECT COUNT(DISTINCT b.content_hash) FROM shared_conflicts c
        JOIN cloudkit_share_inbox_blob_refs r ON r.share_id = c.share_id AND r.operation_id IN (c.local_operation_id, c.remote_operation_id)
        JOIN cloudkit_share_inbox_blobs b ON b.share_id = r.share_id AND b.content_hash = r.content_hash", [], |row| row.get(0)).expect("both retained attachments");
    assert_eq!(count, 2);
}

#[test]
fn foreign_share_uuid_reuse_creates_distinct_local_cards() {
    let (a, owner, _) = owner();
    let initial = batch(&a, &owner);
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    receive(&b, &guest, &initial);
    let first = imported(&b);
    let foreign_board = PinboardId::new();
    let foreign = b
        .register_accepted_pinboard_share(
            foreign_board,
            "Other share",
            "#34c759",
            &format!("PasteShare_{foreign_board}"),
            "another-owner",
            "another-share",
            "https://www.icloud.com/share/synthetic",
            PinboardSharePermission::ReadOnly,
        )
        .expect("another accepted share");
    let mut records = initial
        .operations
        .iter()
        .map(|op| op.envelope.clone())
        .collect::<Vec<_>>();
    for envelope in &mut records {
        match &mut envelope.payload {
            Some(SyncPayload::Pinboard(board)) => {
                board.id = foreign_board;
                envelope.change.entity.id = foreign_board.to_string();
            }
            Some(SyncPayload::PinboardMembership(member)) => {
                member.pinboard_id = foreign_board;
                envelope.change.entity.id = format!("{foreign_board}:{}", member.clip_id);
            }
            _ => {}
        }
    }
    b.stage_pinboard_share_sync_page(&foreign, &records, &initial.blobs, b"foreign")
        .expect("same operation and clip UUIDs, different scope");
    b.materialize_pinboard_share(foreign.id, 250)
        .expect("foreign isolated projection");
    let cards = b.list_history(SearchPage::default()).expect("cards");
    assert_eq!(cards.len(), 2);
    let second = cards
        .iter()
        .find(|c| c.id != first)
        .expect("different binding")
        .id;
    b.rename_clip(first, "First share only")
        .expect("first writable");
    assert!(matches!(
        b.rename_clip(second, "denied"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(
        b.get_clip(second)
            .expect("foreign unchanged")
            .expect("visible")
            .title,
        "shared original"
    );
}
