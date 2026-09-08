use super::*;
use paste_domain::ContentKind;
use paste_storage::{SharedConflictId, SharedConflictResolution as Choice};
use paste_sync::{SyncChangeKind, VersionOrder};

#[test]
fn successive_choices_merge_prior_decisions_and_do_not_acknowledge_private_deliveries() {
    let (a, owner, original) = owner();
    let board = owner.pinboard_id.expect("board");
    let peers = (0..3)
        .map(|_| participant(board, PinboardSharePermission::ReadWrite))
        .collect::<Vec<_>>();
    for (peer, share) in &peers {
        receive(peer, share, &batch(&a, &owner));
    }
    ack(&a, &owner);
    a.update_textual_clip(original, ContentKind::Text, "Version A", "A")
        .expect("A");
    for ((peer, share), title) in peers.iter().zip(["Version B", "Version C", "Version D"]) {
        peer.update_textual_clip(imported(peer), ContentKind::Text, title, title)
            .expect("independent edit");
        receive(&a, &owner, &batch(peer, share));
    }
    let cards = a.list_shared_conflicts(100).expect("three conflicts");
    assert_eq!(cards.len(), 3);
    let card = cards
        .iter()
        .find(|c| c.first.title == "Version A" && c.second.title == "Version B")
        .expect("A/B pair");
    let private_before = a.pending_sync_changes(250).expect("pending private IDs");
    a.resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseFirst)
        .expect("first decision");
    let first = kind(&batch(&a, &owner), SyncEntityKind::Clip);
    let remaining = a.list_shared_conflicts(100).expect("C still unmerged");
    assert!(!remaining.is_empty());
    let card = &remaining[0];
    a.resolve_shared_conflict(card.id, card.current_operation_id, Choice::KeepCurrent)
        .expect("merge remaining history");
    let final_batch = batch(&a, &owner);
    assert_eq!(
        kind(&final_batch, SyncEntityKind::Clip)
            .change
            .version
            .compare(&first.change.version),
        VersionOrder::After
    );
    assert_eq!(
        a.shared_conflict_count()
            .expect("all versions incorporated"),
        0
    );
    let private_after = a
        .pending_sync_changes(250)
        .expect("private queue preserved");
    assert!(private_before.iter().all(|before| {
        private_after
            .iter()
            .any(|after| after.operation_id == before.operation_id)
    }));
    for (peer, share) in &peers {
        receive(peer, share, &final_batch);
        assert_eq!(
            peer.get_clip(imported(peer))
                .expect("peer card")
                .expect("visible")
                .title,
            "Version A"
        );
        assert_eq!(peer.shared_conflict_count().expect("peer resolved"), 0);
    }
}

#[test]
fn retained_native_representations_restore_exact_types_and_bytes_with_bounded_previews() {
    let (a, owner, _) = owner();
    let text = "😀原始格式".repeat(200);
    let original = vec![
        CapturedRepresentation {
            kind: paste_domain::RepresentationKind::PlainText,
            native_type: Some("public.utf16-external-plain-text".into()),
            mime_type: Some("text/plain; charset=utf-16le".into()),
            file_name: None,
            bytes: text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        },
        CapturedRepresentation {
            kind: paste_domain::RepresentationKind::Rtf,
            native_type: Some("public.rtf".into()),
            mime_type: Some("text/rtf".into()),
            file_name: None,
            bytes: br"{\rtf1\ansi original}".to_vec(),
        },
        CapturedRepresentation {
            kind: paste_domain::RepresentationKind::Custom("com.example.synthetic".into()),
            native_type: Some("com.example.synthetic".into()),
            mime_type: None,
            file_name: None,
            bytes: vec![0, 255, 1, 2, 3],
        },
    ];
    let item = a
        .insert_capture(&CapturedItem {
            representations: original.clone(),
            ..capture("unused")
        })
        .expect("synthetic original representations");
    a.pin_clip(owner.pinboard_id.expect("board"), item.id)
        .expect("share original");
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    receive(&b, &guest, &batch(&a, &owner));
    ack(&a, &owner);
    let local = b
        .list_history(SearchPage::default())
        .expect("history")
        .into_iter()
        .find(|clip| clip.content_kind == ContentKind::RichText)
        .expect("mapped rich text");
    let converted = vec![
        CapturedRepresentation {
            native_type: Some("public.utf8-plain-text".into()),
            ..CapturedRepresentation::plain_text("converted")
        },
        CapturedRepresentation {
            kind: paste_domain::RepresentationKind::Rtf,
            native_type: Some("public.rtf".into()),
            mime_type: Some("text/rtf".into()),
            file_name: None,
            bytes: br"{\rtf1 converted}".to_vec(),
        },
    ];
    b.update_rich_text_clip(local.id, local.content_hash, &converted)
        .expect("different edited representations");
    a.rename_clip(item.id, "Retained native aliases")
        .expect("concurrent original");
    receive(&b, &guest, &batch(&a, &owner));
    let card = b.list_shared_conflicts(100).expect("bounded previews")[0].clone();
    assert_eq!(card.second.title, "Retained native aliases");
    assert_eq!(card.second.preview.chars().count(), 512);
    b.resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseSecond)
        .expect("choose untouched native formats");
    assert_eq!(
        b.load_clipboard_payload(local.id)
            .expect("all raw representations"),
        original
    );
    receive(&a, &owner, &batch(&b, &guest));
    assert_eq!(
        a.load_clipboard_payload(item.id).expect("raw roundtrip"),
        original
    );
}

fn conflict() -> (
    SqliteStore,
    PinboardShare,
    ClipId,
    SqliteStore,
    PinboardShare,
    ClipId,
) {
    let (a, owner, original) = owner();
    let (b, guest) = participant(
        owner.pinboard_id.expect("board"),
        PinboardSharePermission::ReadWrite,
    );
    receive(&b, &guest, &batch(&a, &owner));
    ack(&a, &owner);
    let local = imported(&b);
    a.update_textual_clip(original, ContentKind::Text, "Version A", "A exact bytes")
        .expect("A");
    b.update_textual_clip(local, ContentKind::Text, "Version B", "B exact bytes")
        .expect("B");
    let first = batch(&a, &owner);
    let second = batch(&b, &guest);
    receive(&a, &owner, &second);
    receive(&b, &guest, &first);
    (a, owner, original, b, guest, local)
}

#[test]
fn each_choice_generates_a_causal_version_and_converges_on_peers_without_repeating() {
    for choice in [Choice::KeepCurrent, Choice::UseFirst, Choice::UseSecond] {
        let (a, owner, original, b, guest, local) = conflict();
        let card = b.list_shared_conflicts(100).expect("list")[0].clone();
        assert!(card.can_resolve);
        assert_eq!(
            card.id
                .to_string()
                .parse::<SharedConflictId>()
                .expect("opaque ID"),
            card.id
        );
        let chosen = match choice {
            Choice::KeepCurrent => &card.current,
            Choice::UseFirst => &card.first,
            Choice::UseSecond => &card.second,
        };
        assert!(
            b.resolve_shared_conflict(card.id, card.current_operation_id, choice)
                .expect("choice")
        );
        assert!(b.list_shared_conflicts(100).expect("resolved").is_empty());
        let outgoing = batch(&b, &guest);
        assert_eq!(
            outgoing.operations.len(),
            1,
            "older shared deliveries superseded"
        );
        assert_eq!(
            b.get_clip(local).expect("chosen").expect("exists").title,
            chosen.title
        );
        assert_eq!(
            b.load_clipboard_payload(local).expect("exact bytes")[0].bytes,
            chosen.preview.as_bytes()
        );
        assert_eq!(b.pending_sync_change_count().expect("no private echo"), 0);
        assert!(
            !b.resolve_shared_conflict(card.id, card.current_operation_id, choice)
                .expect("idempotent")
        );
        assert_eq!(batch(&b, &guest), outgoing);
        receive(&a, &owner, &outgoing);
        assert_eq!(
            a.get_clip(original).expect("peer").expect("exists").title,
            chosen.title
        );
        assert_eq!(a.shared_conflict_count().expect("peer resolution"), 0);
        let private = a.prepare_private_sync_batch(250).expect("owner fanout");
        let Some(SyncPayload::Clip(item)) = kind(&private, SyncEntityKind::Clip).payload else {
            panic!("private clip")
        };
        assert_eq!(item.title, chosen.title);
    }
}

#[test]
fn preview_revision_prevents_overwriting_a_later_edit_and_keep_current_uses_latest() {
    let (_, _, _, b, guest, local) = conflict();
    let old = b.list_shared_conflicts(100).expect("preview")[0].clone();
    b.update_textual_clip(local, ContentKind::Text, "Later current", "new bytes")
        .expect("later edit");
    let before = batch(&b, &guest);
    assert!(matches!(
        b.resolve_shared_conflict(old.id, old.current_operation_id, Choice::UseFirst),
        Err(StorageError::StaleSharedConflict)
    ));
    assert_eq!(batch(&b, &guest), before);
    let fresh = b.list_shared_conflicts(100).expect("fresh")[0].clone();
    assert_eq!(fresh.current.title, "Later current");
    b.resolve_shared_conflict(fresh.id, fresh.current_operation_id, Choice::KeepCurrent)
        .expect("latest choice");
    assert_eq!(
        b.load_clipboard_payload(local).expect("latest kept")[0].bytes,
        b"new bytes"
    );
    assert_eq!(b.shared_conflict_count().expect("resolved"), 0);
}

#[test]
fn keep_current_after_delete_and_restoring_a_retained_version_preserve_tombstone_causality() {
    for restore in [false, true] {
        let (a, owner, original, b, guest, local) = conflict();
        b.delete_clip(local, Utc::now()).expect("later deletion");
        let deleted = batch(&b, &guest);
        let card = b.list_shared_conflicts(100).expect("delete preview")[0].clone();
        assert!(card.current.deleted);
        b.resolve_shared_conflict(
            card.id,
            card.current_operation_id,
            if restore {
                Choice::UseFirst
            } else {
                Choice::KeepCurrent
            },
        )
        .expect("choice after delete");
        let resolved = batch(&b, &guest);
        let change = &resolved.operations[0].envelope.change;
        assert_eq!(
            change
                .version
                .compare(&kind(&deleted, SyncEntityKind::Clip).change.version),
            VersionOrder::After
        );
        assert_eq!(
            change.change,
            if restore {
                SyncChangeKind::Save
            } else {
                SyncChangeKind::Delete
            }
        );
        receive(&a, &owner, &resolved);
        assert_eq!(a.get_clip(original).expect("peer state").is_some(), restore);
        assert_eq!(a.shared_conflict_count().expect("resolved peer"), 0);
    }
}

#[test]
fn permission_change_foreign_identity_and_invalid_limits_never_commit_a_choice() {
    let (_, _, _, b, guest, local) = conflict();
    let card = b.list_shared_conflicts(100).expect("preview")[0].clone();
    let before = batch(&b, &guest);
    let foreign = SharedConflictId {
        share_id: uuid::Uuid::new_v4(),
        ..card.id
    };
    assert!(matches!(
        b.resolve_shared_conflict(foreign, card.current_operation_id, Choice::UseFirst),
        Err(StorageError::NotFound)
    ));
    for invalid in [0, 101, u32::MAX] {
        assert!(b.list_shared_conflicts(invalid).is_err());
    }
    for invalid in [
        "",
        "not/an/id",
        "00000000-0000-0000-0000-000000000000/extra",
        "a/b/c/d",
    ] {
        assert!(invalid.parse::<SharedConflictId>().is_err());
    }
    let board = guest.pinboard_id.expect("board");
    b.register_accepted_pinboard_share(
        board,
        "Invitation",
        "#34c759",
        &guest.zone_name,
        guest.owner_name.as_deref().expect("owner"),
        guest.share_record_name.as_deref().expect("share"),
        guest.share_url.as_deref().expect("URL"),
        PinboardSharePermission::ReadOnly,
    )
    .expect("permission change");
    assert!(!b.list_shared_conflicts(100).expect("readonly preview")[0].can_resolve);
    assert!(matches!(
        b.resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseFirst),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(b.shared_conflict_count().expect("retained"), 1);
    assert_eq!(
        b.get_clip(local).expect("unchanged").expect("exists").title,
        card.current.title
    );
    assert!(!before.operations.is_empty());
}

#[test]
fn choices_survive_v17_migration_backup_restart_and_release_only_unreferenced_conflict_bytes() {
    let (_, _, _, b, guest, _) = conflict();
    let dir = tempfile::tempdir().expect("synthetic directory");
    let path = dir.path().join("conflict.sqlite");
    b.export_backup(&path).expect("backup");
    let legacy = rusqlite::Connection::open(&path).expect("isolated fixture");
    legacy
        .execute_batch("DROP TABLE shared_conflict_decisions; PRAGMA user_version = 17;")
        .expect("real v17 structure");
    drop(legacy);
    let migrated = SqliteStore::open(&path).expect("v18 migration");
    let card = migrated
        .list_shared_conflicts(100)
        .expect("preserved previews")[0]
        .clone();
    migrated
        .resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseFirst)
        .expect("choice");
    let outgoing = batch(&migrated, &guest);
    drop(migrated);
    let reopened = SqliteStore::open(&path).expect("reopen");
    assert!(
        reopened
            .list_shared_conflicts(100)
            .expect("resolved")
            .is_empty()
    );
    assert!(
        !reopened
            .resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseSecond)
            .expect("no second decision")
    );
    assert_eq!(batch(&reopened, &guest), outgoing);
    let conn = rusqlite::Connection::open(&path).expect("synthetic read-only inspection");
    let count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM cloudkit_share_inbox_blobs WHERE share_id = ?1",
            [guest.id.to_string()],
            |row| row.get(0),
        )
        .expect("unreferenced versions released");
    assert_eq!(count, 1);
    assert_eq!(outgoing.blobs.len(), 1);
}

#[test]
fn resolving_shared_history_after_owner_unpin_does_not_restore_or_overwrite_private_original() {
    let (a, owner, original, _, _, _) = conflict();
    a.unpin_clip(owner.pinboard_id.expect("board"), original)
        .expect("unpin");
    a.rename_clip(original, "Private untouched")
        .expect("private edit");
    let private_before = a.prepare_private_sync_batch(250).expect("private queue");
    let card = a.list_shared_conflicts(100).expect("retained conflict")[0].clone();
    a.resolve_shared_conflict(card.id, card.current_operation_id, Choice::UseFirst)
        .expect("shared-only choice");
    assert_eq!(
        a.get_clip(original)
            .expect("private")
            .expect("exists")
            .title,
        "Private untouched"
    );
    assert_eq!(
        a.prepare_private_sync_batch(250)
            .expect("private queue untouched"),
        private_before
    );
    assert_eq!(a.list_pinboards().expect("still empty")[0].item_count, 0);
}
