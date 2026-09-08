use std::{collections::BTreeMap, path::Path};

use chrono::{Duration, Utc};
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ContentKind, DeviceId, DeviceMetadata,
    RetentionPolicy, SearchFilters, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::SqliteStore;
use rusqlite::{Connection, types::Value};

fn capture(text: &str, age: i64) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now() - Duration::seconds(age),
        source: SourceApplication {
            bundle_identifier: "io.synthetic.editor".into(),
            display_name: "OldEditor".into(),
        },
        device: DeviceMetadata {
            id: DeviceId::from_uuid(uuid::Uuid::nil()),
            display_name: "OldDevice".into(),
        },
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        filters: SearchFilters::default(),
    }
}

fn assert_projection(path: &Path) {
    let db = Connection::open(path).expect("synthetic database");
    let mismatches: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM clips c
        LEFT JOIN clip_search_documents d ON d.id=c.id
        LEFT JOIN clip_search s ON s.rowid=d.doc_id
        WHERE c.deleted_at_ms IS NULL AND (d.doc_id IS NULL OR s.rowid IS NULL
        OR s.clip_id IS NOT c.id OR d.last_copied_at_ms IS NOT c.last_copied_at_ms
        OR d.content_kind IS NOT c.content_kind OR d.source_bundle_id IS NOT c.source_bundle_id
        OR d.device_id IS NOT c.device_id OR s.title IS NOT c.title
        OR s.body IS NOT c.searchable_text OR s.app_name IS NOT c.source_display_name
        OR s.device_name IS NOT c.device_display_name)",
            [],
            |row| row.get(0),
        )
        .expect("synthetic fixture operation");
    assert_eq!(
        mismatches, 0,
        "every live record has exact search metadata and text"
    );
    let extras: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM clip_search_documents d
        LEFT JOIN clips c ON c.id=d.id WHERE c.id IS NULL OR c.deleted_at_ms IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .expect("synthetic fixture operation");
    assert_eq!(extras, 0, "deleted records are not searchable");
    let orphaned: i64 = db.query_row("SELECT COUNT(*) FROM clip_search s
        LEFT JOIN clip_search_documents d ON d.doc_id=s.rowid WHERE d.doc_id IS NULL OR d.id IS NOT s.clip_id", [], |r| r.get(0)).expect("synthetic fixture operation");
    assert_eq!(orphaned, 0, "no stale FTS document identities");
    db.execute(
        "INSERT INTO clip_search(clip_search) VALUES('integrity-check')",
        [],
    )
    .expect("FTS internal integrity");
}

fn cells(db: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let tables = db
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table'
        AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'clip_search%' ORDER BY name",
        )
        .expect("synthetic fixture operation")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("synthetic fixture operation")
        .collect::<Result<Vec<_>, _>>()
        .expect("synthetic fixture operation");
    tables
        .into_iter()
        .map(|name| {
            let quoted = name.replace('"', "\"\"");
            let columns = db
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .expect("synthetic fixture operation")
                .column_count();
            let order = (1..=columns)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let values = db
                .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order}"))
                .expect("synthetic fixture operation")
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| r.get(i))
                        .collect::<Result<Vec<Value>, _>>()
                })
                .expect("synthetic fixture operation")
                .collect::<Result<Vec<_>, _>>()
                .expect("synthetic fixture operation");
            (name, values)
        })
        .collect()
}

fn make_v18(path: &Path) {
    let db = Connection::open(path).expect("synthetic fixture operation");
    db.execute_batch(
        "DROP TRIGGER clips_search_insert; DROP TRIGGER clips_search_remove;
        DROP TRIGGER clips_search_soft_delete; DROP TRIGGER clips_search_restore;
        DROP TRIGGER clips_search_metadata; DROP TRIGGER clips_search_text;
        DROP TABLE clip_search_documents; PRAGMA user_version=18;",
    )
    .expect("synthetic fixture operation");
}

#[test]
fn v18_migration_preserves_every_authoritative_cell_and_original_payload() {
    let temporary = tempfile::tempdir().expect("synthetic fixture operation");
    let path = temporary.path().join("history.sqlite3");
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    let kept = store
        .insert_capture(&capture("durable needle", 400))
        .expect("synthetic fixture operation");
    let deleted = store
        .insert_capture(&capture("deleted needle", 20))
        .expect("synthetic fixture operation");
    let board = store
        .create_pinboard("Protected", "#007aff")
        .expect("synthetic fixture operation");
    store
        .pin_clip(board.id, kept.id)
        .expect("synthetic fixture operation");
    store
        .rename_clip(kept.id, "Renamed title")
        .expect("synthetic fixture operation");
    store
        .delete_clips(&[deleted.id], Utc::now())
        .expect("synthetic fixture operation");
    let payload = store
        .load_clipboard_payload(kept.id)
        .expect("synthetic fixture operation");
    drop(store);
    make_v18(&path);
    let before = cells(&Connection::open(&path).expect("synthetic fixture operation"));
    let store = SqliteStore::open(&path).expect("atomic v18 to v19");
    assert_eq!(
        cells(&Connection::open(&path).expect("synthetic fixture operation")),
        before
    );
    assert_eq!(
        store
            .load_clipboard_payload(kept.id)
            .expect("synthetic fixture operation"),
        payload
    );
    assert_eq!(
        store
            .search_items(&query("needle"), SearchPage::default())
            .expect("synthetic fixture operation")
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [kept.id]
    );
    assert_projection(&path);
    store
        .undo_last_delete_batch()
        .expect("synthetic fixture operation");
    assert_projection(&path);
    assert_eq!(
        store
            .search_items(&query("deleted"), SearchPage::default())
            .expect("synthetic fixture operation")[0]
            .id,
        deleted.id
    );
}

#[test]
fn projection_tracks_edit_recopy_metadata_soft_delete_undo_and_hard_retention() {
    let temporary = tempfile::tempdir().expect("synthetic fixture operation");
    let path = temporary.path().join("history.sqlite3");
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    let mut original = capture("needle original", 300);
    let item = store
        .insert_capture(&original)
        .expect("synthetic fixture operation");
    assert_projection(&path);
    store
        .rename_clip(item.id, "retitled")
        .expect("synthetic fixture operation");
    assert_projection(&path);
    original.captured_at = Utc::now();
    original.source = SourceApplication {
        bundle_identifier: "io.synthetic.new".into(),
        display_name: "NewEditor".into(),
    };
    original.device = DeviceMetadata {
        id: DeviceId::new(),
        display_name: "NewDevice".into(),
    };
    assert_eq!(
        store
            .insert_capture(&original)
            .expect("synthetic fixture operation")
            .id,
        item.id
    );
    assert_projection(&path);
    assert!(
        store
            .search_items(&query("OldEditor"), SearchPage::default())
            .expect("synthetic fixture operation")
            .is_empty()
    );
    assert_eq!(
        store
            .search_items(&query("NewEditor NewDevice"), SearchPage::default())
            .expect("synthetic fixture operation")[0]
            .id,
        item.id
    );
    store
        .update_textual_clip(
            item.id,
            ContentKind::Link,
            "replacement",
            "https://example.com/replaced",
        )
        .expect("synthetic fixture operation");
    assert_projection(&path);
    assert!(
        store
            .search_items(&query("needle"), SearchPage::default())
            .expect("synthetic fixture operation")
            .is_empty()
    );
    assert_eq!(
        store
            .search_items(&query("replacement"), SearchPage::default())
            .expect("synthetic fixture operation")[0]
            .content_kind,
        ContentKind::Link
    );
    store
        .delete_clips(&[item.id], Utc::now())
        .expect("synthetic fixture operation");
    assert_projection(&path);
    store
        .undo_last_delete_batch()
        .expect("synthetic fixture operation");
    assert_projection(&path);
    assert_eq!(
        store
            .apply_retention(
                RetentionPolicy {
                    max_age_days: Some(1),
                    max_unpinned_items: None
                },
                Utc::now() + Duration::days(3)
            )
            .expect("synthetic fixture operation"),
        1
    );
    assert_projection(&path);
}

#[test]
fn projection_and_fts_roll_back_with_failed_writes_and_failed_migration() {
    let temporary = tempfile::tempdir().expect("synthetic fixture operation");
    let path = temporary.path().join("history.sqlite3");
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    store
        .insert_capture(&capture("needle before", 0))
        .expect("synthetic fixture operation");
    drop(store);
    let db = Connection::open(&path).expect("synthetic fixture operation");
    let before = cells(&db);
    db.execute_batch("BEGIN; UPDATE clips SET searchable_text='uncommitted';")
        .expect("synthetic fixture operation");
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM clip_search WHERE clip_search MATCH 'uncommitted'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .expect("synthetic fixture operation"),
        1
    );
    db.execute_batch("ROLLBACK;")
        .expect("synthetic fixture operation");
    assert_eq!(cells(&db), before);
    drop(db);
    assert_projection(&path);
    make_v18(&path);
    let db = Connection::open(&path).expect("synthetic fixture operation");
    // Fault injection after derived FTS is cleared, before it is rebuilt.
    db.execute_batch(
        include_str!("../migrations/0019_search_documents.sql")
            .split("-- Rebuild")
            .next()
            .expect("synthetic fixture operation"),
    )
    .expect("synthetic fixture operation");
    db.execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON clip_search_documents BEGIN SELECT RAISE(ABORT, 'injected migration failure'); END;").expect("synthetic fixture operation");
    assert!(SqliteStore::open(&path).is_err());
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .expect("synthetic fixture operation"),
        18
    );
    assert_eq!(cells(&db), before);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM clip_search WHERE clip_search MATCH 'needle'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .expect("synthetic fixture operation"),
        1,
        "old FTS restored on migration rollback"
    );
    db.execute_batch("DROP TRIGGER injected_failure;")
        .expect("synthetic fixture operation");
    drop(db);
    SqliteStore::open(&path).expect("retry after external failure resolved");
    assert_projection(&path);
}

#[test]
fn document_ids_and_search_survive_vacuum_restart_and_backup_restore() {
    let temporary = tempfile::tempdir().expect("synthetic fixture operation");
    let path = temporary.path().join("history.sqlite3");
    let backup = temporary.path().join("backup.sqlite3");
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    for i in 0..40 {
        store
            .insert_capture(&capture(&format!("needle {i}"), i))
            .expect("synthetic fixture operation");
    }
    store
        .apply_retention(
            RetentionPolicy {
                max_age_days: None,
                max_unpinned_items: Some(7),
            },
            Utc::now(),
        )
        .expect("synthetic fixture operation");
    let expected = store
        .search_items(&query("needle"), SearchPage::default())
        .expect("synthetic fixture operation");
    let identities = |db: &Connection| {
        db.prepare("SELECT id,doc_id FROM clip_search_documents ORDER BY id")
            .expect("synthetic fixture operation")
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .expect("synthetic fixture operation")
            .collect::<Result<Vec<_>, _>>()
            .expect("synthetic fixture operation")
    };
    let before = identities(&Connection::open(&path).expect("synthetic fixture operation"));
    drop(store);
    let db = Connection::open(&path).expect("synthetic fixture operation");
    db.execute_batch("VACUUM;")
        .expect("synthetic fixture operation");
    assert_eq!(identities(&db), before);
    drop(db);
    let store = SqliteStore::open(&path).expect("synthetic fixture operation");
    assert_eq!(
        store
            .search_items(&query("needle"), SearchPage::default())
            .expect("synthetic fixture operation"),
        expected
    );
    store
        .export_backup(&backup)
        .expect("synthetic fixture operation");
    let restored = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    restored
        .restore_backup(&backup)
        .expect("synthetic fixture operation");
    assert_eq!(
        restored
            .search_items(&query("needle"), SearchPage::default())
            .expect("synthetic fixture operation"),
        expected
    );
    assert_projection(&path);
}

#[test]
fn items_path_equals_ranked_path_with_filters_ties_recopy_and_deep_pages() {
    let store = SqliteStore::open_in_memory().expect("synthetic fixture operation");
    let board = store
        .create_pinboard("Ordered", "#007aff")
        .expect("synthetic fixture operation");
    let base = Utc::now() - Duration::days(2);
    let captures = (0..420)
        .map(|i| {
            let mut item = capture(&format!("needle 项目 {i}"), 0);
            // Deliberately non-monotonic timestamps; doc IDs are NOT recency.
            item.captured_at = base + Duration::seconds((i * 47) % 101);
            if i % 3 == 0 {
                item.source.bundle_identifier = "io.synthetic.alternate".into();
            }
            item
        })
        .collect::<Vec<_>>();
    let items = store
        .insert_captures(&captures)
        .expect("synthetic fixture operation");
    store
        .pin_clips(
            board.id,
            &items
                .iter()
                .rev()
                .take(100)
                .map(|c| c.id)
                .collect::<Vec<_>>(),
        )
        .expect("synthetic fixture operation");
    let mut recopied = captures[0].clone();
    recopied.captured_at = Utc::now();
    store
        .insert_capture(&recopied)
        .expect("synthetic fixture operation");
    assert_eq!(
        store
            .search_items(&query("needle"), SearchPage::new(1, 0))
            .expect("synthetic fixture operation")[0]
            .id,
        items[0].id,
        "old document ID promoted by recopy"
    );
    for text in [
        "",
        "needle",
        "needle 项目",
        "notpresent",
        "\"",
        "needle OR 项目",
    ] {
        for filters in [
            SearchFilters::default(),
            SearchFilters {
                pinboard_ids: vec![board.id],
                ..Default::default()
            },
            SearchFilters {
                content_kinds: vec![ContentKind::Text],
                source_bundle_ids: vec!["io.synthetic.alternate".into()],
                device_ids: vec![captures[0].device.id],
                copied_after: Some(base + Duration::seconds(30)),
                copied_before: Some(base + Duration::seconds(70)),
                ..Default::default()
            },
        ] {
            let q = SearchQuery {
                text: text.into(),
                filters,
            };
            for offset in [0, 1, 199, 200, 400] {
                let page = SearchPage::new(200, offset);
                let ranked = store
                    .search(&q, page)
                    .expect("synthetic fixture operation")
                    .into_iter()
                    .map(|hit| hit.item)
                    .collect::<Vec<_>>();
                assert_eq!(
                    store
                        .search_items(&q, page)
                        .expect("synthetic fixture operation"),
                    ranked,
                    "{text}, offset={offset}"
                );
            }
        }
    }
}
