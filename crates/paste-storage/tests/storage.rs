use std::io::Cursor;

use chrono::{Duration, Utc};
use image::{ImageFormat, RgbImage};
use paste_domain::{
    CaptureFlags, CapturePreferences, CapturedItem, CapturedRepresentation, ClipId, ContentKind,
    DesktopPreferences, DeviceId, DeviceMetadata, RepresentationKind, RetentionPolicy,
    SearchFilters, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::{
    CachedPreview, PinboardSharePermission, PinboardShareRole, PinboardShareState, SqliteStore,
    StorageError, SyncConflictResolution,
};
use paste_sync::{SyncChangeKind, SyncEntityKind, SyncPayload, VersionOrder};

fn capture(text: &str, seconds_ago: i64) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now() - Duration::seconds(seconds_ago),
        source: SourceApplication {
            bundle_identifier: "com.example.editor".into(),
            display_name: "Example Editor".into(),
        },
        device: DeviceMetadata {
            id: DeviceId::from_uuid(uuid::Uuid::nil()),
            display_name: "Test Mac".into(),
        },
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

fn acknowledge_all_private(store: &SqliteStore) {
    let ids = store
        .pending_sync_changes(250)
        .expect("pending changes")
        .iter()
        .map(|change| change.operation_id)
        .collect::<Vec<_>>();
    store
        .acknowledge_sync_changes(&ids)
        .expect("acknowledge private snapshots");
}

#[test]
fn persists_searches_and_renames_text() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let item = store
        .insert_capture(&capture("A release checklist for the clipboard app", 1))
        .expect("insert capture");

    let hits = store
        .search(
            &SearchQuery {
                text: "release check".into(),
                filters: SearchFilters::default(),
            },
            SearchPage::default(),
        )
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, item.id);

    store.rename_clip(item.id, "Launch notes").expect("rename");
    let renamed = store.get_clip(item.id).expect("load").expect("exists");
    assert_eq!(renamed.title, "Launch notes");
}

#[test]
fn new_pdf_uses_available_filename_without_losing_text_bytes_or_custom_recapture_title() {
    let store = SqliteStore::open_in_memory().expect("synthetic store");
    let mut pdf = capture("Readable PDF body", 1);
    pdf.representations.push(CapturedRepresentation {
        kind: RepresentationKind::Pdf,
        native_type: Some("com.adobe.pdf".into()),
        mime_type: Some("application/pdf".into()),
        file_name: Some(" /synthetic/path/Quarterly-fixture.pdf ".into()),
        bytes: b"%PDF-1.4 synthetic capture payload".to_vec(),
    });
    let item = store.insert_capture(&pdf).expect("capture PDF metadata");
    assert_eq!(item.title, "Quarterly-fixture.pdf");
    assert_eq!(item.content_kind, ContentKind::Pdf);
    assert_eq!(item.searchable_text, "Readable PDF body");
    for word in ["Quarterly", "Readable"] {
        let hits = store
            .search_items(
                &SearchQuery {
                    text: word.into(),
                    ..Default::default()
                },
                SearchPage::default(),
            )
            .expect("filename and body indexed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, item.id);
    }
    assert_eq!(
        store
            .load_clipboard_payload(item.id)
            .expect("original representations"),
        pdf.representations
    );
    store
        .rename_clip(item.id, "My chosen title")
        .expect("custom title");
    pdf.captured_at = Utc::now();
    assert_eq!(
        store.insert_capture(&pdf).expect("recapture").title,
        "My chosen title"
    );

    let mut unnamed = pdf.clone();
    unnamed.representations[1].file_name = None;
    unnamed.representations[1].bytes.push(b'2');
    assert_eq!(
        store.insert_capture(&unnamed).expect("unnamed PDF").title,
        "Readable PDF body"
    );
}

#[test]
fn text_search_is_newest_first_even_when_older_match_has_better_rank() {
    let store = SqliteStore::open_in_memory().expect("isolated store");
    let older_capture = capture(&"needle ".repeat(30), 300);
    let older = store.insert_capture(&older_capture).expect("older match");
    let newer = store
        .insert_capture(&capture(&format!("needle {}", "padding ".repeat(100)), 100))
        .expect("newer weaker match");
    let query = SearchQuery {
        text: "needle".into(),
        filters: SearchFilters::default(),
    };
    let hits = store.search(&query, SearchPage::default()).expect("search");
    assert_eq!(
        hits.iter().map(|hit| hit.item.id).collect::<Vec<_>>(),
        [newer.id, older.id]
    );
    assert!(
        hits[1].rank < hits[0].rank,
        "fixture must favor the older BM25 match"
    );

    let mut copied_again = older_capture;
    copied_again.captured_at = Utc::now();
    let recopied = store.insert_capture(&copied_again).expect("recopy");
    assert_eq!(recopied.id, older.id);
    let hits = store
        .search(&query, SearchPage::default())
        .expect("recopy order");
    assert_eq!(
        hits.iter().map(|hit| hit.item.id).collect::<Vec<_>>(),
        [older.id, newer.id]
    );
}

#[test]
fn text_search_keeps_stable_ties_pagination_and_matches_beyond_recent_history() {
    let store = SqliteStore::open_in_memory().expect("isolated store");
    let timestamp = Utc::now() - Duration::days(40);
    let mut expected = Vec::new();
    for index in 0..7 {
        let mut item = capture(&format!("archivedneedle {index}"), 0);
        item.captured_at = timestamp;
        expected.push(store.insert_capture(&item).expect("old match").id);
    }
    expected.sort_by_key(|id| std::cmp::Reverse(id.to_string()));
    let recent = (0..250)
        .map(|i| capture(&format!("unrelated recent {i}"), 0))
        .collect::<Vec<_>>();
    store
        .insert_captures(&recent)
        .expect("more than one recent page");
    let query = SearchQuery {
        text: "archivedneedle".into(),
        filters: SearchFilters::default(),
    };
    let mut paged = Vec::new();
    for offset in [0, 3, 6, 9] {
        paged.extend(
            store
                .search(&query, SearchPage::new(3, offset))
                .expect("page")
                .into_iter()
                .map(|hit| hit.item.id),
        );
    }
    assert_eq!(
        paged, expected,
        "search cannot be limited to the recent history page"
    );
}

#[test]
fn single_board_browsing_keeps_manual_order_and_all_combined_filters() {
    let store = SqliteStore::open_in_memory().expect("isolated store");
    let board = store
        .create_pinboard("Ordered filters", "#007aff")
        .expect("board");
    let base = Utc::now() - Duration::hours(1);
    let device = DeviceId::new();
    let mut captures = (0..7)
        .map(|i| {
            let mut item = capture(&format!("needle item {i}"), 0);
            item.captured_at = base + Duration::seconds(i);
            item.device.id = device;
            item
        })
        .collect::<Vec<_>>();
    captures[3].source.bundle_identifier = "com.example.other".into();
    captures[4].device.id = DeviceId::new();
    captures[5].representations = vec![CapturedRepresentation::plain_text(
        "https://example.com/needle",
    )];
    let items = store.insert_captures(&captures).expect("fixtures");
    // Deliberately neither chronological nor reverse chronological.
    store
        .pin_clips(
            board.id,
            &[
                items[2].id,
                items[0].id,
                items[1].id,
                items[3].id,
                items[4].id,
                items[5].id,
                items[6].id,
            ],
        )
        .expect("manual order");
    let mut query = SearchQuery {
        text: String::new(),
        filters: SearchFilters {
            content_kinds: vec![ContentKind::Text],
            source_bundle_ids: vec!["com.example.editor".into()],
            device_ids: vec![device],
            pinboard_ids: vec![board.id],
            copied_after: Some(base),
            copied_before: Some(base + Duration::seconds(5)),
        },
    };
    let ids = |query: &SearchQuery, page| {
        store
            .search(query, page)
            .expect("filtered board")
            .into_iter()
            .map(|hit| hit.item.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&query, SearchPage::default()),
        [items[2].id, items[0].id, items[1].id]
    );
    assert_eq!(ids(&query, SearchPage::new(1, 1)), [items[0].id]);
    query.text = "needle".into();
    assert_eq!(
        ids(&query, SearchPage::default()),
        [items[2].id, items[1].id, items[0].id],
        "text search uses recency, not board position"
    );
    query.filters.copied_after = Some(base + Duration::seconds(1));
    query.filters.copied_before = Some(base + Duration::seconds(1));
    assert_eq!(
        ids(&query, SearchPage::default()),
        [items[1].id],
        "date bounds are inclusive"
    );
    query.text.clear();
    assert_eq!(ids(&query, SearchPage::default()), [items[1].id]);
}

#[test]
fn multiple_board_search_has_no_duplicate_hits_and_preserves_recency() {
    let store = SqliteStore::open_in_memory().expect("isolated store");
    let first = store.create_pinboard("First", "#007aff").expect("board");
    let second = store.create_pinboard("Second", "#007aff").expect("board");
    let older = store
        .insert_capture(&capture("needle older", 100))
        .expect("older");
    let newer = store
        .insert_capture(&capture("needle newer", 0))
        .expect("newer");
    store
        .insert_capture(&capture("needle not pinned", 0))
        .expect("unmatched board");
    store
        .pin_clip(first.id, older.id)
        .expect("first membership");
    store
        .pin_clip(second.id, newer.id)
        .expect("second membership");
    for text in ["", "needle"] {
        let query = SearchQuery {
            text: text.into(),
            filters: SearchFilters {
                pinboard_ids: vec![first.id, second.id, first.id],
                ..Default::default()
            },
        };
        let hits = store
            .search(&query, SearchPage::default())
            .expect("union search");
        assert_eq!(
            hits.iter().map(|hit| hit.item.id).collect::<Vec<_>>(),
            [newer.id, older.id]
        );
        assert_eq!(
            store.search(&query, SearchPage::new(1, 1)).expect("page")[0]
                .item
                .id,
            older.id
        );
    }
}

#[test]
fn unicode_title_limits_keep_captures_renames_and_edits_valid_for_private_sync() {
    let store = SqliteStore::open_in_memory().expect("isolated source");
    let peer = SqliteStore::open_in_memory().expect("isolated peer");
    let long_text = "😀原始格式".repeat(200);
    let title: String = long_text.chars().take(79).chain(['…']).collect();
    let item = store
        .insert_capture(&capture(&long_text, 0))
        .expect("capture");
    assert_eq!(item.title, title);
    assert_eq!(item.searchable_text, long_text);
    for step in 0..4 {
        match step {
            1 => store.rename_clip(item.id, &long_text).expect("long rename"),
            2 | 3 => {
                store
                    .update_textual_clip(
                        item.id,
                        ContentKind::Text,
                        if step == 2 { &long_text } else { "" },
                        &long_text,
                    )
                    .expect("explicit or automatic title on edit");
            }
            _ => {}
        }
        let batch = store
            .prepare_private_sync_batch(250)
            .expect("immutable batch");
        let envelopes = batch
            .operations
            .iter()
            .map(|op| op.envelope.clone())
            .collect::<Vec<_>>();
        peer.apply_remote_sync_batch(&envelopes, &batch.blobs)
            .expect("valid title at peer");
        let received = peer
            .get_clip(item.id)
            .expect("peer query")
            .expect("peer item");
        assert_eq!(received.title, title);
        assert_eq!(received.searchable_text, long_text);
        assert_eq!(
            peer.load_clipboard_payload(item.id).expect("raw bytes"),
            vec![CapturedRepresentation::plain_text(&long_text)]
        );
        acknowledge_all_private(&store);
    }
}

#[test]
fn creates_and_transactionally_edits_textual_items_without_stale_search_or_blobs() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let source = SourceApplication {
        bundle_identifier: "io.pasters.desktop".into(),
        display_name: "PasteRS".into(),
    };
    let device = DeviceMetadata {
        id: DeviceId::new(),
        display_name: "Test Mac".into(),
    };
    let created = store
        .create_textual_item(
            ContentKind::Text,
            "https://example.com/kept-as-text",
            source.clone(),
            device.clone(),
        )
        .expect("create explicit text");
    assert_eq!(created.content_kind, ContentKind::Text);

    let board = store
        .create_pinboard("Editing", "#ff9500")
        .expect("pinboard");
    store
        .pin_clip(board.id, created.id)
        .expect("pin edited item");
    let edited = store
        .update_textual_clip(
            created.id,
            ContentKind::Link,
            "Rust link",
            "https://www.rust-lang.org/learn",
        )
        .expect("edit as link");
    assert_eq!(edited.id, created.id);
    assert_eq!(edited.content_kind, ContentKind::Link);
    assert_eq!(edited.title, "Rust link");
    assert_eq!(edited.representations.len(), 2);
    assert_eq!(edited.representations[0].kind, RepresentationKind::Url);
    assert_eq!(
        store
            .blob_count()
            .expect("old attachment retained for sync"),
        2
    );
    acknowledge_all_private(&store);
    assert_eq!(store.blob_count().expect("blob count"), 1);
    assert_eq!(
        store.list_pinboards().expect("list pinboards")[0].item_count,
        1
    );

    let old_hits = store
        .search(
            &SearchQuery {
                text: "kept-as-text".into(),
                filters: SearchFilters::default(),
            },
            SearchPage::default(),
        )
        .expect("old search");
    assert!(old_hits.is_empty());
    let new_hits = store
        .search(
            &SearchQuery {
                text: "rust-lang".into(),
                filters: SearchFilters::default(),
            },
            SearchPage::default(),
        )
        .expect("new search");
    assert_eq!(new_hits.len(), 1);
    assert_eq!(new_hits[0].item.id, created.id);

    let invalid =
        store.update_textual_clip(created.id, ContentKind::Color, "Broken", "not-a-color");
    assert!(matches!(invalid, Err(StorageError::InvalidEditedColor)));
    assert_eq!(
        store
            .get_clip(created.id)
            .expect("load after rejected edit")
            .expect("item remains")
            .content_kind,
        ContentKind::Link
    );

    let color = store
        .create_textual_item(ContentKind::Color, "#AF52DE", source, device)
        .expect("create color");
    assert_eq!(color.content_kind, ContentKind::Color);
    assert_eq!(color.searchable_text, "#af52de");
}

#[test]
fn color_editor_accepts_captured_bare_hex_without_reclassifying_explicit_text() {
    let store = SqliteStore::open_in_memory().expect("store");
    let input = capture("aBcDeF", 0);
    let item = store.insert_capture(&input).expect("color capture");
    assert_eq!(item.content_kind, ContentKind::Color);
    let edited = store
        .update_textual_clip(item.id, ContentKind::Color, "Edited color", "58aD97")
        .expect("bare hex accepted by editor");
    assert_eq!(edited.searchable_text, "#58ad97");
    assert_eq!(
        store
            .load_clipboard_payload(item.id)
            .expect("edited payload"),
        vec![CapturedRepresentation::plain_text("#58ad97")]
    );
    assert!(matches!(
        store.update_textual_clip(item.id, ContentKind::Color, "Invalid", "235442"),
        Err(StorageError::InvalidEditedColor)
    ));
    assert_eq!(
        store.get_clip(item.id).expect("unchanged").expect("exists"),
        edited
    );
    let explicit_text = store
        .create_textual_item(ContentKind::Text, "#ABCDEF", input.source, input.device)
        .expect("explicit text");
    assert_eq!(explicit_text.content_kind, ContentKind::Text);
}

#[test]
fn creates_and_pins_text_in_one_transaction() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let source = SourceApplication {
        bundle_identifier: "io.pasters.mcp".into(),
        display_name: "PasteRS MCP".into(),
    };
    let device = DeviceMetadata {
        id: DeviceId::new(),
        display_name: "Test Mac".into(),
    };
    let board = store
        .create_pinboard("MCP Inbox", "#ff9500")
        .expect("create pinboard");
    let item = store
        .create_textual_item_in_pinboard(
            ContentKind::Text,
            "transactional MCP note",
            source.clone(),
            device.clone(),
            Some(board.id),
        )
        .expect("create and pin");
    assert_eq!(
        store.list_pinboards().expect("list boards")[0].item_count,
        1
    );
    assert_eq!(
        store
            .search(
                &SearchQuery {
                    text: "transactional".into(),
                    filters: SearchFilters {
                        pinboard_ids: vec![board.id],
                        ..SearchFilters::default()
                    },
                },
                SearchPage::default(),
            )
            .expect("search board")[0]
            .item
            .id,
        item.id
    );

    let before = store
        .list_history(SearchPage::default())
        .expect("history before failed write")
        .len();
    assert!(matches!(
        store.create_textual_item_in_pinboard(
            ContentKind::Text,
            "must roll back",
            source,
            device,
            Some(paste_domain::PinboardId::new()),
        ),
        Err(StorageError::NotFound)
    ));
    assert_eq!(
        store
            .list_history(SearchPage::default())
            .expect("history after failed write")
            .len(),
        before
    );
}

#[test]
fn records_transactional_sync_outbox_versions_acknowledgements_and_tokens() {
    let store = SqliteStore::open_in_memory().expect("open database");
    assert!(!store.cloud_sync_enabled().expect("sync defaults off"));
    store
        .set_cloud_sync_enabled(true)
        .expect("persist sync opt-in");
    assert!(store.cloud_sync_enabled().expect("sync opt-in"));
    let device = store
        .get_or_create_device("Sync Test Mac")
        .expect("create local device");
    let mut captured = capture("sync-ready content", 0);
    captured.device = device;
    let clip = store.insert_capture(&captured).expect("insert clip");
    store
        .rename_clip(clip.id, "sync-ready title")
        .expect("rename clip");
    let pinboard = store
        .create_pinboard("Synced", "#34c759")
        .expect("create pinboard");
    store
        .pin_clip(pinboard.id, clip.id)
        .expect("pin synced clip");

    let pending = store
        .pending_sync_changes(250)
        .expect("load pending changes");
    assert_eq!(pending.len(), 4);
    assert_eq!(pending[0].entity.kind, SyncEntityKind::Clip);
    assert_eq!(pending[0].change, SyncChangeKind::Save);
    assert_eq!(pending[1].entity.id, pending[0].entity.id);
    assert_eq!(
        pending[0].version.compare(&pending[1].version),
        VersionOrder::Before
    );
    assert_eq!(pending[2].entity.kind, SyncEntityKind::Pinboard);
    assert_eq!(pending[3].entity.kind, SyncEntityKind::PinboardMembership);
    let prepared = store
        .prepare_private_sync_batch(250)
        .expect("prepare CloudKit batch");
    assert_eq!(prepared.operations.len(), 3);
    assert_eq!(prepared.blobs.len(), 1);
    assert_eq!(
        prepared
            .operations
            .iter()
            .flat_map(|operation| operation.acknowledge_operation_ids.iter())
            .count(),
        4
    );
    let clip_operation = prepared
        .operations
        .iter()
        .find(|operation| operation.envelope.change.entity.kind == SyncEntityKind::Clip)
        .expect("prepared clip");
    assert_eq!(clip_operation.acknowledge_operation_ids.len(), 2);
    assert!(matches!(
        clip_operation.envelope.payload,
        Some(SyncPayload::Clip(ref item)) if item.id == clip.id && item.title == "sync-ready title"
    ));
    assert_eq!(
        prepared.blobs[0].content_hash,
        clip.representations[0].content_hash
    );
    assert!(matches!(
        store.pending_sync_changes(251),
        Err(StorageError::InvalidSyncBatchLimit)
    ));

    let unknown = uuid::Uuid::new_v4();
    assert!(matches!(
        store.acknowledge_sync_changes(&[pending[0].operation_id, unknown]),
        Err(StorageError::UnknownSyncOperation)
    ));
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("pending count after rollback"),
        4
    );
    store
        .acknowledge_sync_changes(
            &pending
                .iter()
                .map(|change| change.operation_id)
                .collect::<Vec<_>>(),
        )
        .expect("acknowledge batch");
    assert_eq!(
        store
            .pending_sync_change_count()
            .expect("pending count after ack"),
        0
    );

    assert!(
        store
            .load_sync_token("private")
            .expect("empty token")
            .is_none()
    );
    store
        .save_sync_token("private", b"opaque-change-token")
        .expect("save private token");
    assert_eq!(
        store.load_sync_token("private").expect("load token"),
        Some(b"opaque-change-token".to_vec())
    );
    assert!(matches!(
        store.save_sync_token("public", b"not allowed"),
        Err(StorageError::InvalidSyncScope)
    ));
}

#[test]
fn applies_remote_snapshots_idempotently_without_echo_and_surfaces_conflicts() {
    let source = SqliteStore::open_in_memory().expect("open source database");
    let source_device = source
        .get_or_create_device("Source Mac")
        .expect("source device");
    let mut source_capture = capture("cross-device clipboard payload", 0);
    source_capture.device = source_device;
    let clip = source.insert_capture(&source_capture).expect("source clip");
    let board = source
        .create_pinboard("Shared ordering", "#34c759")
        .expect("source pinboard");
    source
        .pin_clip(board.id, clip.id)
        .expect("source membership");
    let initial = source
        .prepare_private_sync_batch(250)
        .expect("prepare initial batch");
    let mut initial_envelopes = initial
        .operations
        .iter()
        .map(|operation| operation.envelope.clone())
        .collect::<Vec<_>>();
    initial_envelopes.reverse();

    let target = SqliteStore::open_in_memory().expect("open target database");
    target
        .get_or_create_device("Target Mac")
        .expect("target device");
    let applied = target
        .apply_remote_sync_batch(&initial_envelopes, &initial.blobs)
        .expect("apply initial batch");
    assert_eq!(applied.applied, 3);
    assert!(applied.conflicts.is_empty());
    assert_eq!(
        target
            .load_clipboard_payload(clip.id)
            .expect("target payload")[0]
            .bytes,
        b"cross-device clipboard payload"
    );
    assert_eq!(
        target.list_pinboards().expect("target boards")[0].item_count,
        1
    );
    assert_eq!(
        target
            .pending_sync_change_count()
            .expect("remote apply does not echo"),
        0
    );
    let replayed = target
        .apply_remote_sync_batch(&initial_envelopes, &initial.blobs)
        .expect("replay initial batch");
    assert_eq!(replayed.ignored, 3);
    assert_eq!(replayed.applied, 0);

    target
        .rename_clip(clip.id, "Target edit")
        .expect("target edit");
    source
        .rename_clip(clip.id, "Source edit")
        .expect("source edit");
    let concurrent = source
        .prepare_private_sync_batch(250)
        .expect("prepare concurrent edit");
    let concurrent_envelopes = concurrent
        .operations
        .iter()
        .map(|operation| operation.envelope.clone())
        .collect::<Vec<_>>();
    let conflict = target
        .apply_remote_sync_batch(&concurrent_envelopes, &concurrent.blobs)
        .expect("surface concurrent edit");
    assert_eq!(conflict.applied, 0);
    assert_eq!(conflict.conflicts.len(), 1);
    assert_eq!(
        target
            .pending_sync_conflict_count()
            .expect("persisted conflict count"),
        1
    );
    let conflict_summary = target
        .list_sync_conflicts(10)
        .expect("list conflicts")
        .into_iter()
        .next()
        .expect("one conflict summary");
    assert_eq!(conflict_summary.clip_id, clip.id);
    assert_eq!(conflict_summary.local_title, "Target edit");
    assert_eq!(conflict_summary.remote_title, "Source edit");
    target
        .resolve_sync_conflict(conflict_summary.id, SyncConflictResolution::KeepLocal)
        .expect("keep local conflict version");
    assert_eq!(
        target
            .pending_sync_conflict_count()
            .expect("resolved conflict count"),
        0
    );
    let resolved_local = target
        .prepare_private_sync_batch(250)
        .expect("prepare resolved local version")
        .operations
        .into_iter()
        .find(|operation| {
            operation.envelope.change.entity.kind == SyncEntityKind::Clip
                && operation.envelope.change.entity.id == clip.id.to_string()
        })
        .expect("resolved local clip operation");
    let remote_clip = concurrent_envelopes
        .iter()
        .find(|envelope| envelope.change.entity.kind == SyncEntityKind::Clip)
        .expect("remote clip envelope");
    assert_eq!(
        resolved_local
            .envelope
            .change
            .version
            .compare(&remote_clip.change.version),
        VersionOrder::After
    );
    assert_eq!(
        target
            .get_clip(clip.id)
            .expect("target clip")
            .expect("target exists")
            .title,
        "Target edit"
    );

    let accept_target = SqliteStore::open_in_memory().expect("open accept target database");
    accept_target
        .get_or_create_device("Accept Mac")
        .expect("accept target device");
    accept_target
        .apply_remote_sync_batch(&initial_envelopes, &initial.blobs)
        .expect("seed accept target");
    accept_target
        .rename_clip(clip.id, "Accept target edit")
        .expect("edit accept target");
    accept_target
        .apply_remote_sync_batch(&concurrent_envelopes, &concurrent.blobs)
        .expect("surface accept conflict");
    let accept_conflict = accept_target
        .list_sync_conflicts(10)
        .expect("list accept conflicts")[0]
        .id;
    accept_target
        .resolve_sync_conflict(accept_conflict, SyncConflictResolution::AcceptRemote)
        .expect("accept remote conflict version");
    assert_eq!(
        accept_target
            .get_clip(clip.id)
            .expect("accept target clip")
            .expect("accept target exists")
            .title,
        "Source edit"
    );
    assert_eq!(
        accept_target
            .pending_sync_change_count()
            .expect("accepted choice replaces stale outbox"),
        1
    );
}

#[test]
fn mcp_access_is_disabled_by_default_scoped_per_client_and_revocable() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let token_hash = *blake3::hash(b"one-time raw client token").as_bytes();
    let client = store
        .register_mcp_client("Codex local", &token_hash)
        .expect("register MCP client");
    assert!(!store.mcp_enabled().expect("MCP default"));
    assert!(
        store
            .authorize_mcp_token(&token_hash)
            .expect("disabled authorization")
            .is_none()
    );

    store.set_mcp_enabled(true).expect("enable MCP");
    let authorized = store
        .authorize_mcp_token(&token_hash)
        .expect("authorize token")
        .expect("authorized client");
    assert_eq!(authorized.id, client.id);
    assert!(authorized.last_used_at.is_some());
    assert_eq!(store.list_mcp_clients().expect("list clients").len(), 1);

    store.revoke_mcp_client(client.id).expect("revoke client");
    assert!(store.list_mcp_clients().expect("list revoked").is_empty());
    assert!(
        store
            .authorize_mcp_token(&token_hash)
            .expect("revoked authorization")
            .is_none()
    );
    assert!(matches!(
        store.register_mcp_client("", &token_hash),
        Err(StorageError::InvalidMcpClientName)
    ));
}

#[test]
fn rotates_image_content_and_indexes_ocr_without_losing_identity_or_membership() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let original_png = png(5, 3, [12, 34, 56]);
    let rotated_png = png(3, 5, [78, 90, 123]);
    let mut image_capture = capture("placeholder", 1);
    image_capture.representations = vec![CapturedRepresentation {
        native_type: None,
        kind: RepresentationKind::Png,
        mime_type: Some("image/png".into()),
        file_name: None,
        bytes: original_png,
    }];
    let image = store
        .insert_capture(&image_capture)
        .expect("insert image capture");
    assert_eq!(image.content_kind, ContentKind::Image);
    let board = store
        .create_pinboard("Images", "#007aff")
        .expect("create image pinboard");
    store.pin_clip(board.id, image.id).expect("pin image item");
    store
        .save_cached_preview(
            image.id,
            &image.content_hash,
            &CachedPreview {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3],
                pixel_width: 5,
                pixel_height: 3,
            },
        )
        .expect("cache original preview");

    let updated = store
        .update_image_content(image.id, &rotated_png)
        .expect("replace image content");
    assert_eq!(updated.id, image.id);
    assert_eq!(updated.content_kind, ContentKind::Image);
    assert_ne!(updated.content_hash, image.content_hash);
    assert_eq!(store.blob_count().expect("old image retained for sync"), 2);
    acknowledge_all_private(&store);
    assert_eq!(store.blob_count().expect("blob count"), 1);
    assert_eq!(
        store.list_pinboards().expect("list pinboards")[0].item_count,
        1
    );
    assert!(
        store
            .load_cached_preview(image.id, &updated.content_hash)
            .expect("load cleared preview")
            .is_none()
    );
    assert!(matches!(
        store.update_image_content(image.id, b"not png"),
        Err(StorageError::InvalidImageEdit)
    ));
    assert_eq!(
        store
            .get_clip(image.id)
            .expect("load after rejected image edit")
            .expect("image remains")
            .content_hash,
        updated.content_hash
    );

    let recognized = store
        .update_ocr_text(image.id, "locally recognized release checklist")
        .expect("store OCR text");
    assert_eq!(recognized.id, image.id);
    assert_eq!(
        recognized.searchable_text,
        "locally recognized release checklist"
    );
    let hits = store
        .search(
            &SearchQuery {
                text: "recognized release".into(),
                filters: SearchFilters::default(),
            },
            SearchPage::default(),
        )
        .expect("search OCR text");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, image.id);
    assert!(matches!(
        store.update_ocr_text(image.id, "  "),
        Err(StorageError::EmptyOcrText)
    ));

    let text = store
        .insert_capture(&capture("not an image", 0))
        .expect("insert text");
    assert!(matches!(
        store.update_ocr_text(text.id, "must not be stored"),
        Err(StorageError::InvalidImageEdit)
    ));
}

fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let image = RgbImage::from_pixel(width, height, image::Rgb(color));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode test PNG");
    bytes.into_inner()
}

#[test]
fn coalesces_identical_content_and_reuses_blobs() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let first = store
        .insert_capture(&capture("same content", 2))
        .expect("first capture");
    let second = store
        .insert_capture(&capture("same content", 0))
        .expect("second capture");

    assert_eq!(first.id, second.id);
    assert!(second.last_copied_at > first.last_copied_at);
    assert_eq!(
        store
            .list_history(SearchPage::default())
            .expect("list history")
            .len(),
        1
    );
    assert_eq!(store.blob_count().expect("count blobs"), 1);
}

#[test]
fn filters_by_content_kind() {
    let store = SqliteStore::open_in_memory().expect("open database");
    store
        .insert_capture(&capture("https://pasteapp.io/help", 1))
        .expect("link capture");
    store
        .insert_capture(&capture("ordinary note", 0))
        .expect("text capture");

    let hits = store
        .search(
            &SearchQuery {
                text: String::new(),
                filters: SearchFilters {
                    content_kinds: vec![ContentKind::Link],
                    ..SearchFilters::default()
                },
            },
            SearchPage::default(),
        )
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.content_kind, ContentKind::Link);
}

#[test]
fn pinned_items_survive_history_retention() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let old_pinned = store
        .insert_capture(&capture("keep me", 10_000))
        .expect("old capture");
    let recent = store
        .insert_capture(&capture("recent", 0))
        .expect("recent capture");
    let pinboard = store
        .create_pinboard("Useful", "#FF9F0A")
        .expect("pinboard");
    store
        .pin_clip(pinboard.id, old_pinned.id)
        .expect("pin old item");

    let removed = store
        .apply_retention(
            RetentionPolicy {
                max_age_days: None,
                max_unpinned_items: Some(1),
            },
            Utc::now(),
        )
        .expect("retention");
    assert_eq!(removed, 0);
    assert!(
        store
            .get_clip(old_pinned.id)
            .expect("load pinned")
            .is_some()
    );
    assert!(store.get_clip(recent.id).expect("load recent").is_some());
    assert_eq!(
        store.list_pinboards().expect("list pinboards")[0].item_count,
        1
    );
}

#[test]
fn edits_and_reorders_pinboards_atomically() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let first = store
        .create_pinboard("First", "#FF9500")
        .expect("first pinboard");
    let second = store
        .create_pinboard("Second", "#34C759")
        .expect("second pinboard");
    let third = store
        .create_pinboard("Third", "#007AFF")
        .expect("third pinboard");

    let updated = store
        .update_pinboard(second.id, "Renamed", "#AF52DE")
        .expect("update pinboard");
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.color, "#af52de");

    let reordered = store
        .reorder_pinboards(&[third.id, second.id, first.id])
        .expect("reorder pinboards");
    assert_eq!(
        reordered.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![third.id, second.id, first.id]
    );
    assert_eq!(
        reordered
            .iter()
            .map(|item| item.sort_order)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let invalid = store.reorder_pinboards(&[first.id]);
    assert!(matches!(invalid, Err(StorageError::InvalidPinboardOrder)));
    assert_eq!(
        store
            .list_pinboards()
            .expect("list after rejected reorder")
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![third.id, second.id, first.id]
    );
}

#[test]
fn pinboard_item_order_is_visible_movable_and_board_deletion_keeps_history() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let newer = store
        .insert_capture(&capture("newer", 0))
        .expect("newer capture");
    let older = store
        .insert_capture(&capture("older", 10))
        .expect("older capture");
    let board = store
        .create_pinboard("Ordered", "#ff9500")
        .expect("pinboard");
    store
        .pin_clips(board.id, &[older.id, newer.id])
        .expect("pin in explicit order");

    let query = SearchQuery {
        text: String::new(),
        filters: SearchFilters {
            pinboard_ids: vec![board.id],
            ..SearchFilters::default()
        },
    };
    let initial = store
        .search(&query, SearchPage::default())
        .expect("ordered search");
    assert_eq!(
        initial.iter().map(|hit| hit.item.id).collect::<Vec<_>>(),
        vec![older.id, newer.id]
    );

    assert!(
        store
            .move_pinboard_item(board.id, newer.id, -1)
            .expect("move newer first")
    );
    assert!(
        !store
            .move_pinboard_item(board.id, newer.id, -1)
            .expect("already first")
    );
    let moved = store
        .search(&query, SearchPage::default())
        .expect("moved search");
    assert_eq!(
        moved.iter().map(|hit| hit.item.id).collect::<Vec<_>>(),
        vec![newer.id, older.id]
    );

    store.delete_pinboard(board.id).expect("delete board");
    assert!(store.list_pinboards().expect("list boards").is_empty());
    assert!(store.get_clip(newer.id).expect("newer remains").is_some());
    assert!(store.get_clip(older.id).expect("older remains").is_some());
}

#[test]
fn rejects_confidential_capture_before_writing() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let mut item = capture("secret", 0);
    item.flags.confidential = true;

    assert!(matches!(
        store.insert_capture(&item),
        Err(StorageError::CaptureRejected)
    ));
    assert!(
        store
            .list_history(SearchPage::default())
            .expect("list history")
            .is_empty()
    );
}

#[test]
fn keeps_a_stable_device_identity_across_reopens() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("history.db");
    let first_id = {
        let store = SqliteStore::open(&database).expect("first open");
        store
            .get_or_create_device("First name")
            .expect("first identity")
            .id
    };
    let store = SqliteStore::open(&database).expect("second open");
    let second = store
        .get_or_create_device("Renamed Mac")
        .expect("second identity");

    assert_eq!(second.id, first_id);
    assert_eq!(second.display_name, "Renamed Mac");
}

#[test]
fn restores_exact_clipboard_representation_bytes() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let mut captured = capture("plain version", 0);
    captured.representations.push(CapturedRepresentation {
        native_type: None,
        kind: paste_domain::RepresentationKind::Html,
        mime_type: Some("text/html; charset=utf-8".into()),
        file_name: None,
        bytes: b"<strong>rich version</strong>".to_vec(),
    });
    let item = store.insert_capture(&captured).expect("insert capture");

    let restored = store
        .load_clipboard_payload(item.id)
        .expect("restore payload");
    assert_eq!(restored, captured.representations);
}

#[test]
fn caches_previews_by_source_content_hash() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let item = store
        .insert_capture(&capture("preview source", 0))
        .expect("insert capture");
    let preview = CachedPreview {
        media_type: "image/png".into(),
        bytes: vec![1, 2, 3, 4],
        pixel_width: 320,
        pixel_height: 180,
    };

    store
        .save_cached_preview(item.id, &item.content_hash, &preview)
        .expect("save preview");
    assert_eq!(
        store
            .load_cached_preview(item.id, &item.content_hash)
            .expect("load preview"),
        Some(preview)
    );

    let stale_hash = [9; 32];
    assert_eq!(
        store
            .load_cached_preview(item.id, &stale_hash)
            .expect("load stale preview"),
        None
    );
}

#[test]
fn upgrades_a_version_one_database_without_losing_history() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("history.db");
    let connection = rusqlite::Connection::open(&database).expect("create legacy database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("apply version one schema");
    drop(connection);

    let store = SqliteStore::open(&database).expect("upgrade database");
    let item = store
        .insert_capture(&capture("survives migration", 0))
        .expect("insert after migration");
    assert_eq!(
        store
            .get_clip(item.id)
            .expect("load clip")
            .expect("clip exists")
            .searchable_text,
        "survives migration"
    );
    assert!(
        !store
            .get_or_create_device("Migrated Mac")
            .expect("device settings")
            .display_name
            .is_empty()
    );
}

#[test]
fn upgrades_a_version_nine_database_through_conflict_and_share_storage() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("history.db");
    drop(SqliteStore::open(&database).expect("create current database"));

    let previous = rusqlite::Connection::open(&database).expect("open downgrade fixture");
    previous
        .execute_batch(
            "ALTER TABLE representations DROP COLUMN native_type;
             ALTER TABLE sync_outbox DROP COLUMN envelope_schema_version;
             DROP TABLE sync_deferred_memberships;
             DROP TABLE sync_outbox_blob_refs;
             DROP TABLE sync_outbox_snapshots;
             DROP TABLE cloudkit_share_outbox;
             DROP TABLE cloudkit_shares;
             DROP TABLE sync_conflict_blobs;
             PRAGMA user_version = 9;",
        )
        .expect("create version nine fixture");
    drop(previous);

    drop(SqliteStore::open(&database).expect("upgrade version nine database"));
    let upgraded = rusqlite::Connection::open(&database).expect("inspect upgraded database");
    let version: u32 = upgraded
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read upgraded schema version");
    let conflict_blob_table: u32 = upgraded
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'sync_conflict_blobs'",
            [],
            |row| row.get(0),
        )
        .expect("inspect conflict blob table");
    let share_table: u32 = upgraded
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'cloudkit_shares'",
            [],
            |row| row.get(0),
        )
        .expect("inspect CloudKit share table");
    let share_outbox_table: u32 = upgraded
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'cloudkit_share_outbox'",
            [],
            |row| row.get(0),
        )
        .expect("inspect shared outbox table");
    assert_eq!(version, 19);
    assert_eq!(conflict_blob_table, 1);
    assert_eq!(share_table, 1);
    assert_eq!(share_outbox_table, 1);
}

#[test]
fn persists_owned_and_accepted_pinboard_share_lifecycles() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let owned_board = store
        .create_pinboard("Team snippets", "#ff9500")
        .expect("create owned board");
    let reserved = store
        .reserve_owned_pinboard_share(owned_board.id, PinboardSharePermission::ReadWrite)
        .expect("reserve owned share");
    assert_eq!(reserved.role, PinboardShareRole::Owner);
    assert_eq!(reserved.state, PinboardShareState::Preparing);
    assert!(reserved.zone_name.starts_with("PasteShare_"));
    assert_eq!(
        store
            .reserve_owned_pinboard_share(owned_board.id, PinboardSharePermission::ReadWrite)
            .expect("idempotent reservation")
            .id,
        reserved.id
    );

    let active = store
        .activate_owned_pinboard_share(
            owned_board.id,
            "__defaultOwner__",
            "CKRecordNameZoneWideShare",
            "https://www.icloud.com/share/example",
        )
        .expect("activate owned share");
    assert_eq!(active.state, PinboardShareState::Active);
    assert_eq!(active.permission, PinboardSharePermission::ReadWrite);
    assert_eq!(
        store
            .pending_pinboard_share_change_count(active.id)
            .expect("pending initial shared snapshot"),
        1
    );
    let shared_batch = store
        .prepare_pinboard_share_sync_batch(active.id, 250)
        .expect("prepare initial shared snapshot");
    assert_eq!(shared_batch.operations.len(), 1);
    assert!(
        shared_batch
            .operations
            .iter()
            .all(|operation| operation.envelope.scope == paste_sync::SyncScope::Shared)
    );
    let shared_acknowledgements = shared_batch
        .operations
        .iter()
        .flat_map(|operation| operation.acknowledge_operation_ids.iter().copied())
        .collect::<Vec<_>>();
    store
        .acknowledge_pinboard_share_changes(active.id, &shared_acknowledgements)
        .expect("acknowledge shared snapshot");
    assert_eq!(
        store
            .pending_pinboard_share_change_count(active.id)
            .expect("shared snapshot acknowledged"),
        0
    );
    store
        .update_pinboard(owned_board.id, "Team snippets updated", "#ff9500")
        .expect("owner can update shared board");
    assert_eq!(
        store
            .pending_pinboard_share_change_count(active.id)
            .expect("owner update routed to share"),
        1
    );
    assert!(
        store
            .list_pinboards()
            .expect("list boards")
            .into_iter()
            .find(|board| board.id == owned_board.id)
            .expect("owned board")
            .is_shared
    );
    store
        .stage_pinboard_share_sync_page(&active, &[], &[], b"opaque-shared-zone-token")
        .expect("save share token");

    let accepted_id = paste_domain::PinboardId::new();
    let accepted = store
        .register_accepted_pinboard_share(
            accepted_id,
            "Remote research",
            "#34c759",
            &format!("PasteShare_{accepted_id}"),
            "owner_record_name",
            "CKRecordNameZoneWideShare",
            "https://www.icloud.com/share/accepted",
            PinboardSharePermission::ReadOnly,
        )
        .expect("register accepted share");
    assert_eq!(accepted.role, PinboardShareRole::Participant);
    assert_eq!(accepted.permission, PinboardSharePermission::ReadOnly);
    assert_eq!(accepted.pinboard_id, Some(accepted_id));
    assert!(matches!(
        store.update_pinboard(accepted_id, "Blocked", "#34c759"),
        Err(StorageError::ReadOnlyPinboardShare)
    ));
    assert_eq!(store.list_pinboard_shares().expect("list shares").len(), 2);
    assert!(matches!(
        store.register_accepted_pinboard_share(
            owned_board.id,
            "Cannot replace owner",
            "#34c759",
            "PasteShare_other",
            "owner_record_name",
            "CKRecordNameZoneWideShare",
            "https://www.icloud.com/share/conflict",
            PinboardSharePermission::ReadOnly,
        ),
        Err(StorageError::PinboardAlreadyShared)
    ));
}

#[test]
fn normalizes_and_persists_capture_preferences() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let saved = store
        .save_capture_preferences(CapturePreferences {
            retention: RetentionPolicy {
                max_age_days: Some(30),
                max_unpinned_items: Some(2_000),
            },
            excluded_bundle_ids: vec![
                " COM.EXAMPLE.Passwords ".into(),
                "com.example.passwords".into(),
            ],
        })
        .expect("save preferences");

    assert_eq!(saved.excluded_bundle_ids, vec!["com.example.passwords"]);
    assert_eq!(
        store.load_capture_preferences().expect("load preferences"),
        saved
    );
}

#[test]
fn persists_desktop_preferences() {
    let store = SqliteStore::open_in_memory().expect("open database");
    assert_eq!(
        store.load_desktop_preferences().expect("load defaults"),
        DesktopPreferences::default()
    );

    let expected = DesktopPreferences {
        language: paste_domain::Language::Chinese,
        launch_at_login: true,
        screen_share_protection: true,
        compact_mode: true,
    };
    assert_eq!(
        store
            .save_desktop_preferences(expected)
            .expect("save preferences"),
        expected
    );
    assert_eq!(
        store.load_desktop_preferences().expect("load preferences"),
        expected
    );
}

#[test]
fn soft_delete_can_be_undone_with_search_index_restored() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let item = store
        .insert_capture(&capture("undoable release note", 0))
        .expect("insert capture");
    store.delete_clip(item.id, Utc::now()).expect("soft delete");
    assert!(store.get_clip(item.id).expect("get deleted").is_none());
    assert!(
        store
            .search(
                &SearchQuery {
                    text: "undoable".into(),
                    filters: SearchFilters::default(),
                },
                SearchPage::default(),
            )
            .expect("search deleted")
            .is_empty()
    );

    let restored = store
        .undo_last_delete()
        .expect("undo delete")
        .expect("restored item");
    assert_eq!(restored.id, item.id);
    assert_eq!(
        store
            .search(
                &SearchQuery {
                    text: "undoable".into(),
                    filters: SearchFilters::default(),
                },
                SearchPage::default(),
            )
            .expect("search restored")
            .len(),
        1
    );
}

#[test]
fn batch_delete_is_atomic_and_undo_restores_the_whole_batch() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let first = store
        .insert_capture(&capture("first batch item", 2))
        .expect("insert first");
    let second = store
        .insert_capture(&capture("second batch item", 1))
        .expect("insert second");

    let missing = ClipId::new();
    assert!(matches!(
        store.delete_clips(&[first.id, missing], Utc::now()),
        Err(StorageError::NotFound)
    ));
    assert!(
        store
            .get_clip(first.id)
            .expect("first after rollback")
            .is_some()
    );

    store
        .delete_clips(&[first.id, second.id], Utc::now())
        .expect("delete batch");
    assert!(store.get_clip(first.id).expect("first deleted").is_none());
    assert!(store.get_clip(second.id).expect("second deleted").is_none());

    let restored = store.undo_last_delete_batch().expect("undo batch delete");
    assert_eq!(restored.len(), 2);
    assert!(store.get_clip(first.id).expect("first restored").is_some());
    assert!(
        store
            .get_clip(second.id)
            .expect("second restored")
            .is_some()
    );
}

#[test]
fn batch_pin_and_unpin_updates_pinboard_membership() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let first = store
        .insert_capture(&capture("first pinned item", 2))
        .expect("insert first");
    let second = store
        .insert_capture(&capture("second pinned item", 1))
        .expect("insert second");
    let pinboard = store
        .create_pinboard("Batch", "#ff9500")
        .expect("create pinboard");

    store
        .pin_clips(pinboard.id, &[first.id, second.id])
        .expect("pin batch");
    assert_eq!(
        store.list_pinboards().expect("list pinboards")[0].item_count,
        2
    );

    store
        .unpin_clips(pinboard.id, &[first.id, second.id])
        .expect("unpin batch");
    assert_eq!(
        store.list_pinboards().expect("list pinboards")[0].item_count,
        0
    );
}

#[test]
fn exports_and_restores_a_versioned_integrity_checked_backup() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let backup = directory.path().join("history.pasters-backup");
    let store = SqliteStore::open_in_memory().expect("open database");
    let original = store
        .insert_capture(&capture("present in backup", 2))
        .expect("insert original");
    store.export_backup(&backup).expect("export backup");

    let later = store
        .insert_capture(&capture("created after backup", 0))
        .expect("insert later");
    store
        .delete_clip(original.id, Utc::now())
        .expect("delete original");
    store.restore_backup(&backup).expect("restore backup");

    assert!(
        store
            .get_clip(original.id)
            .expect("load original")
            .is_some()
    );
    assert!(store.get_clip(later.id).expect("load later").is_none());
    assert_eq!(
        store
            .search(
                &SearchQuery {
                    text: "present in backup".into(),
                    filters: SearchFilters::default(),
                },
                SearchPage::default(),
            )
            .expect("search restored")
            .len(),
        1
    );
}

#[test]
fn rejects_a_sqlite_file_without_the_pasters_backup_identity() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let invalid = directory.path().join("other.sqlite");
    rusqlite::Connection::open(&invalid).expect("create unrelated database");
    let store = SqliteStore::open_in_memory().expect("open database");

    assert!(matches!(
        store.restore_backup(&invalid),
        Err(StorageError::InvalidBackup(_))
    ));
}

#[test]
fn restores_and_migrates_a_previous_schema_backup_in_memory() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let backup = directory.path().join("schema-five.pasters-backup");
    let source = SqliteStore::open_in_memory().expect("open source database");
    let original = source
        .insert_capture(&capture("migrate this backup", 0))
        .expect("insert source item");
    source
        .export_backup(&backup)
        .expect("export current backup");
    drop(source);

    let previous = rusqlite::Connection::open(&backup).expect("open backup for downgrade fixture");
    previous
        .execute_batch(
            "ALTER TABLE representations DROP COLUMN native_type;
             DROP TABLE sync_deferred_memberships;
             DROP TABLE sync_outbox_blob_refs;
             DROP TABLE sync_outbox_snapshots;
             DROP TABLE cloudkit_share_outbox;
             DROP TABLE cloudkit_shares;
             DROP TABLE sync_conflict_blobs;
             DROP TABLE sync_conflicts;
             DROP TABLE sync_tokens;
             DROP TABLE sync_outbox;
             DROP TABLE sync_entity_versions;
             DROP TABLE sync_clocks;
             PRAGMA user_version = 5;",
        )
        .expect("create previous schema fixture");
    drop(previous);

    let destination = SqliteStore::open_in_memory().expect("open destination database");
    destination
        .restore_backup(&backup)
        .expect("restore and migrate previous backup");
    assert!(
        destination
            .get_clip(original.id)
            .expect("load restored item")
            .is_some()
    );
    assert_eq!(
        destination
            .pending_sync_change_count()
            .expect("migrated sync outbox"),
        0
    );
}

#[test]
fn combines_source_device_date_filters_and_reports_facets() {
    let store = SqliteStore::open_in_memory().expect("open database");
    let older = capture("older editor item", 172_800);
    let mut recent = capture("recent browser item", 60);
    recent.source = SourceApplication {
        bundle_identifier: "com.example.browser".into(),
        display_name: "Example Browser".into(),
    };
    recent.device = DeviceMetadata {
        id: DeviceId::new(),
        display_name: "Other Mac".into(),
    };
    let older_item = store.insert_capture(&older).expect("insert older");
    let recent_item = store.insert_capture(&recent).expect("insert recent");

    let hits = store
        .search(
            &SearchQuery {
                text: String::new(),
                filters: SearchFilters {
                    source_bundle_ids: vec![recent.source.bundle_identifier.clone()],
                    device_ids: vec![recent.device.id],
                    copied_after: Some(Utc::now() - Duration::hours(1)),
                    ..SearchFilters::default()
                },
            },
            SearchPage::default(),
        )
        .expect("combined search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.id, recent_item.id);

    let facets = store.list_search_facets().expect("list facets");
    assert_eq!(facets.sources.len(), 2);
    assert!(
        facets
            .sources
            .iter()
            .any(|source| source.bundle_identifier == "com.example.browser")
    );
    assert_eq!(facets.devices.len(), 2);
    assert_eq!(
        store
            .history_position(recent_item.id)
            .expect("recent position"),
        0
    );
    assert_eq!(
        store
            .history_position(older_item.id)
            .expect("older position"),
        1
    );
}

#[test]
fn language_upgrade_roundtrip_isolated_update_and_failed_save() {
    use paste_domain::Language;
    let directory = tempfile::tempdir().expect("temporary store");
    let path = directory.path().join("language.db");
    let store = SqliteStore::open(&path).expect("open");
    let sql = rusqlite::Connection::open(&path).expect("fixture connection");
    // Existing installations have no language key. Keep their Chinese UI and options.
    sql.execute(
        "INSERT INTO settings (key, value, updated_at_ms) VALUES ('desktop_preferences', ?1, 0)",
        [r#"{"launch_at_login":true,"screen_share_protection":true,"compact_mode":true}"#],
    )
    .expect("legacy preferences");
    let previous = store.load_desktop_preferences().expect("legacy defaults");
    assert_eq!(previous.language, Language::Chinese);
    let capture = store
        .load_capture_preferences()
        .expect("capture preferences");
    assert_eq!(
        store
            .save_language(Language::English)
            .expect("save language"),
        Language::English
    );
    drop(store);
    let store = SqliteStore::open(&path).expect("reopen");
    assert_eq!(
        store.load_desktop_preferences().expect("reloaded"),
        DesktopPreferences {
            language: Language::English,
            ..previous
        }
    );
    assert_eq!(
        store.load_capture_preferences().expect("capture unchanged"),
        capture
    );
    // Reject the write at the database boundary: the previously saved choice survives.
    sql.execute_batch("CREATE TRIGGER reject_language BEFORE UPDATE ON settings WHEN NEW.key = 'desktop_preferences' BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;").expect("failure fixture");
    assert!(store.save_language(Language::Chinese).is_err());
    assert_eq!(
        store
            .load_desktop_preferences()
            .expect("unchanged after failure")
            .language,
        Language::English
    );
    for (wire, language) in [("en", Language::English), ("zh-CN", Language::Chinese)] {
        assert_eq!(
            serde_json::from_value::<Language>(serde_json::json!(wire)).expect("language"),
            language
        );
        assert_eq!(serde_json::to_value(language).expect("serialize"), wire);
    }
    assert!(serde_json::from_value::<Language>(serde_json::json!("xx")).is_err());
}
