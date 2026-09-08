use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ContentKind, DeviceId, DeviceMetadata,
    RepresentationKind, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::{SqliteStore, SyncConflictResolution};
use paste_sync::{PreparedSyncBatch, SyncEnvelopeError, SyncPayload};

fn capture(representations: Vec<CapturedRepresentation>) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now(),
        source: SourceApplication::unknown(),
        device: DeviceMetadata {
            id: DeviceId::new(),
            display_name: "Synthetic Mac".into(),
        },
        flags: CaptureFlags::default(),
        representations,
    }
}

fn utf16(text: &str) -> CapturedRepresentation {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    CapturedRepresentation {
        native_type: Some("public.utf16-external-plain-text".into()),
        bytes,
        ..CapturedRepresentation::plain_text("")
    }
}

fn batch(store: &SqliteStore) -> PreparedSyncBatch {
    store
        .prepare_private_sync_batch(250)
        .expect("prepared batch")
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
        .expect("acknowledge");
}
fn receive(store: &SqliteStore, batch: &PreparedSyncBatch) {
    store
        .apply_remote_sync_batch(
            &batch
                .operations
                .iter()
                .map(|op| op.envelope.clone())
                .collect::<Vec<_>>(),
            &batch.blobs,
        )
        .expect("receive batch");
}

#[test]
fn native_aliases_bytes_and_decoded_search_survive_restart_and_backup_restore() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let database = directory.path().join("typed.db");
    let store = SqliteStore::open(&database).expect("store");
    let original = vec![
        utf16("原始格式 🦀"),
        CapturedRepresentation {
            native_type: Some("NSStringPboardType".into()),
            ..CapturedRepresentation::plain_text("another alias")
        },
        CapturedRepresentation {
            kind: RepresentationKind::Rtf,
            native_type: Some("NeXT Rich Text Format v1.0 pasteboard type".into()),
            mime_type: None,
            file_name: None,
            bytes: br"{\rtf1 synthetic}".to_vec(),
        },
    ];
    let item = store
        .insert_capture(&capture(original.clone()))
        .expect("typed capture");
    assert_eq!(item.searchable_text, "原始格式 🦀");
    assert_eq!(
        store
            .search(
                &SearchQuery {
                    text: "原始格式".into(),
                    ..Default::default()
                },
                SearchPage::default()
            )
            .expect("FTS")
            .len(),
        1
    );
    let backup = directory.path().join("typed.pasters-backup");
    store.export_backup(&backup).expect("backup");
    drop(store);
    let store = SqliteStore::open(database).expect("reopen");
    assert_eq!(
        store
            .load_clipboard_payload(item.id)
            .expect("restart payload"),
        original
    );
    let restored = SqliteStore::open_in_memory().expect("destination");
    restored.restore_backup(&backup).expect("restore backup");
    assert_eq!(
        restored
            .load_clipboard_payload(item.id)
            .expect("restored bytes and types"),
        original
    );
}

#[test]
fn copied_color_classification_preserves_raw_payload_and_survives_restart_and_backup() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let database = directory.path().join("colors.db");
    let store = SqliteStore::open(&database).expect("store");
    let mut saved = Vec::new();
    for (text, expected) in [
        ("  1a2B3c\n", ContentKind::Color),
        ("#235442", ContentKind::Color),
        ("235442", ContentKind::Text),
        ("#FFF", ContentKind::Text),
    ] {
        for representation in [
            CapturedRepresentation {
                native_type: Some("public.utf8-plain-text".into()),
                ..CapturedRepresentation::plain_text(text)
            },
            utf16(text),
        ] {
            let original = vec![representation];
            let input = capture(original.clone());
            let item = store.insert_capture(&input).expect("capture");
            assert_eq!(item.content_kind, expected);
            assert_eq!(
                item.searchable_text, text,
                "display parsing must not rewrite stored text"
            );
            let repeated = store.insert_capture(&input).expect("deduplicate");
            assert_eq!(repeated.id, item.id);
            assert_eq!(repeated.content_hash, item.content_hash);
            assert_eq!(
                store.load_clipboard_payload(item.id).expect("payload"),
                original
            );
            saved.push((item, original));
        }
    }
    let backup = directory.path().join("colors.pasters-backup");
    store.export_backup(&backup).expect("backup");
    drop(store);
    let reopened = SqliteStore::open(database).expect("reopen");
    let restored = SqliteStore::open_in_memory().expect("restored store");
    restored.restore_backup(&backup).expect("restore");
    for target in [&reopened, &restored] {
        for (item, original) in &saved {
            assert_eq!(
                target.get_clip(item.id).expect("load").expect("exists"),
                *item
            );
            assert_eq!(
                target.load_clipboard_payload(item.id).expect("raw payload"),
                *original
            );
        }
    }
}

#[test]
fn native_type_is_part_of_clip_identity_but_not_blob_identity() {
    let store = SqliteStore::open_in_memory().expect("store");
    let first = capture(vec![CapturedRepresentation {
        native_type: Some("public.utf8-plain-text".into()),
        ..CapturedRepresentation::plain_text("same bytes")
    }]);
    let second = capture(vec![CapturedRepresentation {
        native_type: Some("NSStringPboardType".into()),
        ..CapturedRepresentation::plain_text("same bytes")
    }]);
    let a = store.insert_capture(&first).expect("first type");
    let b = store.insert_capture(&second).expect("second type");
    assert_ne!(a.id, b.id);
    assert_ne!(a.content_hash, b.content_hash);
    assert_eq!(
        a.representations[0].content_hash,
        b.representations[0].content_hash
    );
    assert_eq!(
        store.insert_capture(&first).expect("repeat same type").id,
        a.id
    );
}

#[test]
fn version_fourteen_migration_preserves_legacy_hash_payload_uuid_and_wire_version() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("legacy.db");
    let store = SqliteStore::open(&path).expect("fixture source");
    let item = store
        .insert_capture(&capture(vec![CapturedRepresentation::plain_text(
            "legacy bytes",
        )]))
        .expect("legacy capture");
    let mut expected = batch(&store).operations[0].envelope.clone();
    expected.schema_version = 1;
    let expected_json = serde_json::to_vec(&expected).expect("legacy v1 wire");
    drop(store);
    let legacy = rusqlite::Connection::open(&path).expect("synthetic downgrade");
    // A real v14 database has no v17 frozen envelopes or receipt journal.
    legacy.execute_batch("DROP TABLE shared_conflicts; DROP TABLE shared_entity_states;
        DROP TABLE shared_operation_receipts; DROP TABLE shared_projection_queue;
        DROP TABLE shared_wire_outbox; DROP TABLE shared_only_outbox; DROP TABLE shared_clip_bindings;
        DROP TABLE cloudkit_share_inbox_blob_refs; DROP TABLE cloudkit_share_inbox;
        DROP TABLE cloudkit_share_inbox_blobs;
        ALTER TABLE representations DROP COLUMN native_type;
        ALTER TABLE sync_outbox DROP COLUMN envelope_schema_version; PRAGMA user_version = 14;").expect("v14 schema");
    drop(legacy);
    let upgraded = SqliteStore::open(path).expect("v15 migration");
    assert_eq!(
        serde_json::to_vec(&batch(&upgraded).operations[0].envelope).expect("retry bytes"),
        expected_json
    );
    let current = upgraded.get_clip(item.id).expect("clip").expect("exists");
    assert_eq!(current.content_hash, item.content_hash);
    assert!(current.representations[0].native_type.is_none());
    upgraded
        .insert_capture(&capture(vec![utf16("new typed record")]))
        .expect("new capture");
    let mixed = batch(&upgraded);
    assert_eq!(
        mixed
            .operations
            .iter()
            .map(|op| op.envelope.schema_version)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn v2_types_roundtrip_but_downgrade_invalid_metadata_and_tampering_do_not_commit() {
    let source = SqliteStore::open_in_memory().expect("source");
    let target = SqliteStore::open_in_memory().expect("target");
    let original = vec![utf16("synchronized 🦀")];
    let item = source
        .insert_capture(&capture(original.clone()))
        .expect("typed capture");
    let prepared = batch(&source);
    let envelope = prepared.operations[0].envelope.clone();
    assert_eq!(envelope.schema_version, 2);
    let mut downgrade = envelope.clone();
    downgrade.schema_version = 1;
    assert!(matches!(
        downgrade.validate(),
        Err(SyncEnvelopeError::NativeTypeRequiresV2)
    ));
    assert!(
        target
            .apply_private_sync_page(&[downgrade], &prepared.blobs, b"bad-downgrade")
            .is_err()
    );
    let mut invalid = envelope.clone();
    if let Some(SyncPayload::Clip(clip)) = &mut invalid.payload {
        clip.representations[0].native_type = Some("bad\0type".into());
    }
    assert!(
        target
            .apply_private_sync_page(&[invalid], &prepared.blobs, b"bad-type")
            .is_err()
    );
    let mut tampered = envelope.clone();
    if let Some(SyncPayload::Clip(clip)) = &mut tampered.payload {
        clip.representations[0].native_type = None;
    }
    assert!(
        target
            .apply_private_sync_page(&[tampered], &prepared.blobs, b"missing-type")
            .is_err()
    );
    assert!(
        target
            .get_clip(item.id)
            .expect("no partial write")
            .is_none()
    );
    assert!(
        target
            .load_sync_token("private")
            .expect("unchanged token")
            .is_none()
    );
    receive(&target, &prepared);
    assert_eq!(
        target
            .load_clipboard_payload(item.id)
            .expect("received raw bytes"),
        original
    );
    assert_eq!(target.pending_sync_change_count().expect("no echo"), 0);
}

#[test]
fn accepting_remote_conflict_restores_original_types_and_queues_a_v2_choice() {
    let source = SqliteStore::open_in_memory().expect("source");
    let local = SqliteStore::open_in_memory().expect("local");
    let original = vec![utf16("original typed data")];
    let item = source
        .insert_capture(&capture(original.clone()))
        .expect("original");
    let first = batch(&source);
    receive(&local, &first);
    ack(&source, &first);
    local
        .update_textual_clip(item.id, ContentKind::Text, "local edited", "plain edit")
        .expect("local edit");
    source
        .rename_clip(item.id, "remote renamed")
        .expect("remote rename");
    receive(&local, &batch(&source));
    let conflict = local.list_sync_conflicts(100).expect("conflicts")[0].id;
    local
        .resolve_sync_conflict(conflict, SyncConflictResolution::AcceptRemote)
        .expect("remote choice");
    assert_eq!(
        local
            .load_clipboard_payload(item.id)
            .expect("remote type restored"),
        original
    );
    let chosen = batch(&local);
    assert!(
        chosen
            .operations
            .iter()
            .all(|op| op.envelope.schema_version == 2)
    );
    let observer = SqliteStore::open_in_memory().expect("observer");
    receive(&observer, &chosen);
    assert_eq!(
        observer
            .load_clipboard_payload(item.id)
            .expect("choice forwarded"),
        original
    );
}

#[test]
fn invalid_native_type_in_capture_is_transactionally_rejected() {
    let store = SqliteStore::open_in_memory().expect("store");
    let bad = capture(vec![CapturedRepresentation {
        native_type: Some("bad\0type".into()),
        ..CapturedRepresentation::plain_text("must not persist")
    }]);
    assert!(store.insert_capture(&bad).is_err());
    assert_eq!(
        store.pending_sync_change_count().expect("no pending write"),
        0
    );
    assert!(
        store
            .search(&SearchQuery::default(), SearchPage::default())
            .expect("no rows")
            .is_empty()
    );
}

#[test]
fn migration_keeps_shared_only_pending_snapshots_on_their_original_wire_version() {
    let directory = tempfile::tempdir().expect("isolated directory");
    let path = directory.path().join("shared-legacy.db");
    let store = SqliteStore::open(&path).expect("source");
    let board = store
        .create_pinboard("Synthetic shared board", "#34c759")
        .expect("board");
    let item = store
        .insert_capture(&capture(vec![CapturedRepresentation::plain_text(
            "shared legacy",
        )]))
        .expect("clip");
    store.pin_clips(board.id, &[item.id]).expect("pin");
    store
        .reserve_owned_pinboard_share(board.id, paste_storage::PinboardSharePermission::ReadWrite)
        .expect("reserve");
    let share = store
        .activate_owned_pinboard_share(
            board.id,
            "test_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
        )
        .expect("activate");
    ack(&store, &batch(&store));
    let mut expected = store
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("shared only pending");
    assert!(!expected.operations.is_empty());
    for op in &mut expected.operations {
        op.envelope.schema_version = 1;
    }
    drop(store);
    let legacy = rusqlite::Connection::open(&path).expect("legacy fixture");
    legacy.execute_batch("DROP TABLE shared_conflicts; DROP TABLE shared_entity_states;
        DROP TABLE shared_operation_receipts; DROP TABLE shared_projection_queue;
        DROP TABLE shared_wire_outbox; DROP TABLE shared_only_outbox; DROP TABLE shared_clip_bindings;
        DROP TABLE cloudkit_share_inbox_blob_refs; DROP TABLE cloudkit_share_inbox;
        DROP TABLE cloudkit_share_inbox_blobs;
        ALTER TABLE representations DROP COLUMN native_type;
        ALTER TABLE sync_outbox DROP COLUMN envelope_schema_version; PRAGMA user_version = 14;").expect("v14 schema");
    drop(legacy);
    let migrated = SqliteStore::open(path).expect("upgrade");
    assert!(batch(&migrated).operations.is_empty());
    let actual = migrated
        .prepare_pinboard_share_sync_batch(share.id, 250)
        .expect("shared retry");
    assert_eq!(actual, expected);
    assert_eq!(
        migrated
            .load_clipboard_payload(item.id)
            .expect("retained attachment")[0]
            .bytes,
        b"shared legacy"
    );
}
