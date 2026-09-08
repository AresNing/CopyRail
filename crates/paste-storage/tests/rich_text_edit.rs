use chrono::Utc;
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ContentKind, DeviceId, DeviceMetadata,
    RepresentationKind, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::{PinboardSharePermission, SqliteStore, StorageError};

fn rich(text: &str) -> Vec<CapturedRepresentation> {
    [
        (
            RepresentationKind::PlainText,
            "public.utf8-plain-text",
            text.as_bytes().to_vec(),
        ),
        (
            RepresentationKind::Rtf,
            "public.rtf",
            format!("{{\\rtf1\\b {text}}}").into_bytes(),
        ),
        (
            RepresentationKind::Html,
            "public.html",
            format!("<b>{text}</b>").into_bytes(),
        ),
    ]
    .into_iter()
    .map(|(kind, native_type, bytes)| CapturedRepresentation {
        kind,
        native_type: Some(native_type.into()),
        bytes,
        mime_type: None,
        file_name: None,
    })
    .collect()
}

fn insert(store: &SqliteStore) -> paste_domain::ClipItem {
    store
        .insert_capture(&CapturedItem {
            captured_at: Utc::now(),
            source: SourceApplication::unknown(),
            device: DeviceMetadata {
                id: DeviceId::new(),
                display_name: "Synthetic Mac".into(),
            },
            flags: CaptureFlags::default(),
            representations: rich("original"),
        })
        .expect("synthetic item")
}

#[test]
fn rich_edit_preserves_identity_latest_title_pinboard_and_restarts_with_search_and_sync() {
    let dir = tempfile::tempdir().expect("isolated directory");
    let path = dir.path().join("rich.db");
    let store = SqliteStore::open(&path).expect("database");
    let original = insert(&store);
    let board = store
        .create_pinboard("Synthetic board", "#34c759")
        .expect("board");
    store.pin_clips(board.id, &[original.id]).expect("pin");
    let snapshot = store
        .rich_text_edit_snapshot(original.id)
        .expect("atomic snapshot");
    assert_eq!(snapshot.representations, rich("original"));
    store
        .rename_clip(original.id, "Renamed while editing")
        .expect("rename");
    let updated = store
        .update_rich_text_clip(original.id, snapshot.item.content_hash, &rich("updated"))
        .expect("save");
    assert_eq!(updated.id, original.id);
    assert_eq!(updated.source, original.source);
    assert_eq!(updated.device, original.device);
    assert_eq!(updated.captured_at, original.captured_at);
    assert_eq!(updated.title, "Renamed while editing");
    assert_eq!(updated.content_kind, ContentKind::RichText);
    assert_eq!(store.list_pinboards().expect("boards")[0].item_count, 1);
    let pending = store.pending_sync_change_count().expect("pending");
    store
        .update_rich_text_clip(original.id, updated.content_hash, &rich("updated"))
        .expect("identical save");
    assert_eq!(
        store.pending_sync_change_count().expect("no extra queue"),
        pending
    );
    drop(store);
    let reopened = SqliteStore::open(path).expect("restart");
    assert_eq!(
        reopened.load_clipboard_payload(original.id).expect("bytes"),
        rich("updated")
    );
    assert_eq!(
        reopened
            .search(
                &SearchQuery {
                    text: "updated".into(),
                    ..Default::default()
                },
                SearchPage::default()
            )
            .expect("search")
            .len(),
        1
    );
    assert!(
        reopened
            .search(
                &SearchQuery {
                    text: "original".into(),
                    ..Default::default()
                },
                SearchPage::default()
            )
            .expect("old index removed")
            .is_empty()
    );
    let batch = reopened
        .prepare_private_sync_batch(250)
        .expect("sync payload");
    let receiver = SqliteStore::open_in_memory().expect("receiver");
    receiver
        .apply_remote_sync_batch(
            &batch
                .operations
                .iter()
                .map(|op| op.envelope.clone())
                .collect::<Vec<_>>(),
            &batch.blobs,
        )
        .expect("receive");
    assert_eq!(
        receiver
            .load_clipboard_payload(original.id)
            .expect("remote bytes"),
        rich("updated")
    );
}

#[test]
fn concurrent_content_change_delete_and_new_readonly_permission_never_get_overwritten() {
    let store = SqliteStore::open_in_memory().expect("store");
    let item = insert(&store);
    store
        .update_textual_clip(item.id, ContentKind::Text, "New content", "newer")
        .expect("concurrent edit");
    let pending = store.pending_sync_change_count().expect("pending");
    assert!(matches!(
        store.update_rich_text_clip(item.id, item.content_hash, &rich("stale")),
        Err(StorageError::StaleContentEdit)
    ));
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("no queue mutation"),
        pending
    );
    let current = store.get_clip(item.id).expect("item").expect("exists");
    store.delete_clip(item.id, Utc::now()).expect("delete");
    assert!(matches!(
        store.update_rich_text_clip(item.id, current.content_hash, &rich("stale")),
        Err(StorageError::NotFound)
    ));
    assert!(store.rich_text_edit_snapshot(item.id).is_err());

    let second = insert(&store);
    let board_id = paste_domain::PinboardId::new();
    store
        .register_accepted_pinboard_share(
            board_id,
            "Synthetic shared",
            "#34c759",
            &format!("PasteShare_{board_id}"),
            "test_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
            PinboardSharePermission::ReadWrite,
        )
        .expect("accept writable");
    store.pin_clips(board_id, &[second.id]).expect("pin");
    let snapshot = store
        .rich_text_edit_snapshot(second.id)
        .expect("open before permission change");
    store
        .register_accepted_pinboard_share(
            board_id,
            "Synthetic shared",
            "#34c759",
            &format!("PasteShare_{board_id}"),
            "test_owner",
            "zone_share",
            "https://www.icloud.com/share/synthetic",
            PinboardSharePermission::ReadOnly,
        )
        .expect("permission change");
    assert!(matches!(
        store.update_rich_text_clip(second.id, snapshot.item.content_hash, &rich("unauthorized")),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert!(matches!(
        store.rich_text_edit_snapshot(second.id),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(
        store.load_clipboard_payload(second.id).expect("unchanged"),
        rich("original")
    );
}

#[test]
fn malformed_oversized_or_empty_edits_cannot_partially_replace_representations() {
    let store = SqliteStore::open_in_memory().expect("store");
    let item = insert(&store);
    let mut bad_type = rich("bad");
    bad_type[1].native_type = Some("public.png".into());
    let mut duplicate = rich("bad");
    duplicate.push(duplicate[0].clone());
    let mut invalid_rtf = rich("bad");
    invalid_rtf[1].bytes = b"not RTF".to_vec();
    let mut oversized = rich("bad");
    oversized[2].bytes = vec![b'x'; 4 * 1024 * 1024];
    let pending = store.pending_sync_change_count().expect("pending");
    for bad in [
        bad_type,
        duplicate,
        invalid_rtf,
        oversized,
        rich(""),
        vec![CapturedRepresentation::plain_text("no formatting")],
    ] {
        assert!(
            store
                .update_rich_text_clip(item.id, item.content_hash, &bad)
                .is_err()
        );
        assert_eq!(
            store
                .load_clipboard_payload(item.id)
                .expect("original bytes"),
            rich("original")
        );
        assert_eq!(
            store.pending_sync_change_count().expect("original pending"),
            pending
        );
    }
}
