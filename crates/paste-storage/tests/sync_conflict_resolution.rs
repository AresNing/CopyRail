use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ClipId, ContentKind, DeviceId,
    DeviceMetadata, PinboardId, SourceApplication,
};
use paste_storage::{PinboardSharePermission, SqliteStore, StorageError, SyncConflictResolution};
use paste_sync::{PreparedSyncBatch, RemoteConflict, SyncChangeKind, SyncPayload, VersionOrder};

fn batch(store: &SqliteStore) -> PreparedSyncBatch {
    store
        .prepare_private_sync_batch(250)
        .expect("prepare synthetic batch")
}

fn receive(target: &SqliteStore, batch: &PreparedSyncBatch) -> Vec<RemoteConflict> {
    target
        .apply_remote_sync_batch(
            &batch
                .operations
                .iter()
                .map(|op| op.envelope.clone())
                .collect::<Vec<_>>(),
            &batch.blobs,
        )
        .expect("receive synthetic batch")
        .conflicts
}

fn ack(store: &SqliteStore, batch: &PreparedSyncBatch) {
    store
        .acknowledge_sync_changes(
            &batch
                .operations
                .iter()
                .flat_map(|op| op.acknowledge_operation_ids.iter().copied())
                .collect::<Vec<_>>(),
        )
        .expect("acknowledge batch");
}

fn devices() -> (SqliteStore, SqliteStore, SqliteStore, ClipId) {
    let first = SqliteStore::open_in_memory().expect("first device");
    let second = SqliteStore::open_in_memory().expect("second device");
    let third = SqliteStore::open_in_memory().expect("third device");
    let clip = first
        .insert_capture(&CapturedItem {
            captured_at: Utc::now(),
            source: SourceApplication::unknown(),
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Synthetic Mac".into(),
            },
            flags: CaptureFlags::default(),
            representations: vec![CapturedRepresentation::plain_text("initial")],
        })
        .expect("initial capture");
    let initial = batch(&first);
    assert!(receive(&second, &initial).is_empty());
    assert!(receive(&third, &initial).is_empty());
    ack(&first, &initial);
    (first, second, third, clip.id)
}

fn edit(store: &SqliteStore, id: ClipId, text: &str) {
    store
        .update_textual_clip(id, ContentKind::Text, text, text)
        .expect("synthetic edit");
}

fn conflict_id(store: &SqliteStore) -> uuid::Uuid {
    store.list_sync_conflicts(100).expect("conflicts")[0].id
}

#[test]
fn keeping_local_uses_the_latest_edit_and_never_rolls_back_its_vector() {
    let (remote, local, observer, id) = devices();
    edit(&remote, id, "remote edit");
    let incoming = batch(&remote);
    edit(&local, id, "local edit when conflict appeared");
    assert_eq!(receive(&local, &incoming).len(), 1);
    let conflict = conflict_id(&local);
    for text in ["later edit one", "later edit two", "latest local edit"] {
        edit(&local, id, text);
    }
    let latest = batch(&local);
    assert!(receive(&observer, &latest).is_empty());
    local
        .resolve_sync_conflict(conflict, SyncConflictResolution::KeepLocal)
        .expect("keep current local state");
    let resolved = batch(&local);
    assert_eq!(resolved.operations.len(), 1);
    let choice = &resolved.operations[0].envelope;
    assert_eq!(
        choice
            .change
            .version
            .compare(&latest.operations[0].envelope.change.version),
        VersionOrder::After
    );
    assert_eq!(
        choice
            .change
            .version
            .compare(&incoming.operations[0].envelope.change.version),
        VersionOrder::After
    );
    assert!(
        matches!(&choice.payload, Some(SyncPayload::Clip(clip)) if clip.title == "latest local edit")
    );
    assert!(receive(&observer, &resolved).is_empty());
    assert_eq!(
        observer
            .get_clip(id)
            .expect("observer")
            .expect("clip")
            .title,
        "latest local edit"
    );
    assert!(receive(&local, &incoming).is_empty());
    assert_eq!(
        local.pending_sync_conflict_count().expect("resolved count"),
        0
    );
}

#[test]
fn keeping_local_after_delete_or_undo_respects_the_current_operation_kind() {
    for keep_deleted in [true, false] {
        let (remote, local, observer, id) = devices();
        edit(&remote, id, "remote edit");
        if keep_deleted {
            edit(&local, id, "local edit");
        } else {
            local
                .delete_clip(id, Utc::now())
                .expect("initial local deletion");
        }
        assert_eq!(receive(&local, &batch(&remote)).len(), 1);
        if keep_deleted {
            local
                .delete_clip(id, Utc::now())
                .expect("delete after conflict");
        } else {
            local.undo_last_delete().expect("undo after conflict");
        }
        local
            .resolve_sync_conflict(conflict_id(&local), SyncConflictResolution::KeepLocal)
            .expect("resolve current local kind");
        let resolved = batch(&local);
        assert_eq!(resolved.operations.len(), 1);
        assert_eq!(
            resolved.operations[0].envelope.change.change,
            if keep_deleted {
                SyncChangeKind::Delete
            } else {
                SyncChangeKind::Save
            }
        );
        assert!(receive(&observer, &resolved).is_empty());
        assert_eq!(
            observer.get_clip(id).expect("observer").is_none(),
            keep_deleted
        );
    }
}

#[test]
fn accepting_remote_creates_a_causal_choice_that_overrides_already_synced_local_edits() {
    for remote_deletes in [false, true] {
        let (remote, local, observer, id) = devices();
        edit(&local, id, "local edit already on third device");
        let local_edit = batch(&local);
        assert!(receive(&observer, &local_edit).is_empty());
        ack(&local, &local_edit);
        if remote_deletes {
            remote.delete_clip(id, Utc::now()).expect("remote deletion");
        } else {
            edit(&remote, id, "chosen remote bytes");
        }
        let incoming = batch(&remote);
        assert_eq!(receive(&local, &incoming).len(), 1);
        let conflict = conflict_id(&local);
        local
            .resolve_sync_conflict(conflict, SyncConflictResolution::AcceptRemote)
            .expect("choose remote");
        let resolved = batch(&local);
        assert_eq!(
            resolved.operations.len(),
            1,
            "choice must be published even if old local edits already reached cloud"
        );
        let choice = &resolved.operations[0].envelope.change;
        assert_ne!(
            choice.operation_id,
            incoming.operations[0].envelope.change.operation_id
        );
        assert_eq!(
            choice
                .version
                .compare(&local_edit.operations[0].envelope.change.version),
            VersionOrder::After
        );
        assert_eq!(
            choice
                .version
                .compare(&incoming.operations[0].envelope.change.version),
            VersionOrder::After
        );
        assert!(receive(&remote, &resolved).is_empty());
        assert!(receive(&observer, &resolved).is_empty());
        for device in [&remote, &local, &observer] {
            let clip = device.get_clip(id).expect("converged clip");
            if remote_deletes {
                assert!(clip.is_none());
            } else {
                assert_eq!(clip.expect("clip").searchable_text, "chosen remote bytes");
            }
        }
        assert!(receive(&local, &local_edit).is_empty());
        assert!(receive(&local, &incoming).is_empty());
        ack(&local, &resolved);
        local
            .resolve_sync_conflict(conflict, SyncConflictResolution::AcceptRemote)
            .expect("retry resolved command");
        assert!(
            batch(&local).operations.is_empty(),
            "retry cannot emit another choice"
        );
    }
}

#[test]
fn resolving_multiple_remote_conflicts_preserves_prior_choices_in_the_vector() {
    let (remote, local, third, id) = devices();
    edit(&remote, id, "remote one");
    edit(&local, id, "local one");
    edit(&third, id, "remote two");
    let first_incoming = batch(&remote);
    let second_incoming = batch(&third);
    receive(&local, &first_incoming);
    receive(&local, &second_incoming);
    assert_eq!(local.pending_sync_conflict_count().expect("count"), 2);
    local
        .resolve_sync_conflict(
            first_incoming.operations[0].envelope.change.operation_id,
            SyncConflictResolution::KeepLocal,
        )
        .expect("first choice");
    let first_choice = batch(&local);
    assert_eq!(
        local.pending_sync_conflict_count().expect("second remains"),
        1
    );
    local
        .resolve_sync_conflict(
            second_incoming.operations[0].envelope.change.operation_id,
            SyncConflictResolution::AcceptRemote,
        )
        .expect("second choice");
    let final_choice = batch(&local);
    assert_eq!(final_choice.operations.len(), 1);
    assert_eq!(
        final_choice.operations[0]
            .envelope
            .change
            .version
            .compare(&first_choice.operations[0].envelope.change.version),
        VersionOrder::After
    );
    assert!(receive(&remote, &final_choice).is_empty());
    assert!(receive(&third, &final_choice).is_empty());
    assert_eq!(
        local.get_clip(id).expect("local").expect("clip").title,
        "remote two"
    );
    assert_eq!(
        local.pending_sync_conflict_count().expect("all resolved"),
        0
    );
}

#[test]
fn readonly_permission_change_also_blocks_accepting_remote_conflict_payloads() {
    let (remote, local, _, id) = devices();
    edit(&remote, id, "remote one");
    edit(&local, id, "local one");
    receive(&local, &batch(&remote));
    let board = PinboardId::new();
    let register = |permission| {
        local
            .register_accepted_pinboard_share(
                board,
                "Synthetic",
                "#34c759",
                &format!("PasteShare_{board}"),
                "test_owner",
                "zone_share",
                "https://www.icloud.com/share/synthetic",
                permission,
            )
            .expect("register permission")
    };
    register(PinboardSharePermission::ReadWrite);
    local.pin_clip(board, id).expect("pin before downgrade");
    register(PinboardSharePermission::ReadOnly);
    let before = batch(&local);
    for resolution in [
        SyncConflictResolution::KeepLocal,
        SyncConflictResolution::AcceptRemote,
    ] {
        assert!(matches!(
            local.resolve_sync_conflict(conflict_id(&local), resolution),
            Err(StorageError::ReadOnlyPinboardShare)
        ));
        assert_eq!(before, batch(&local));
        assert_eq!(
            local.get_clip(id).expect("local").expect("clip").title,
            "local one"
        );
        assert_eq!(
            local
                .pending_sync_conflict_count()
                .expect("still unresolved"),
            1
        );
    }
}
