use chrono::{Duration, Utc};
use paste_core::{CaptureCoordinator, CaptureEvent, CaptureInterruption};
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, DeviceMetadata, RetentionPolicy,
    SearchPage, SourceApplication,
};
use paste_platform::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
};
use paste_storage::SqliteStore;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Source {
    polls: VecDeque<ClipboardPoll>,
    reads: Arc<AtomicUsize>,
}
impl ClipboardSource for Source {
    fn discard_current(&mut self) -> Result<(), ClipboardError> {
        self.polls.clear();
        Ok(())
    }

    fn poll(
        &mut self,
        _: &ClipboardPrivacyPolicy,
        _: CaptureLimits,
        _: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.polls.pop_front().unwrap_or(ClipboardPoll::Unchanged))
    }
}
fn item(device: &DeviceMetadata, text: &str, age: i64) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now() - Duration::seconds(age),
        source: SourceApplication {
            bundle_identifier: "com.synthetic.Editor".into(),
            display_name: "Synthetic Editor".into(),
        },
        device: device.clone(),
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}
fn source(items: Vec<CapturedItem>) -> (Source, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    (
        Source {
            polls: VecDeque::from([ClipboardPoll::Captured {
                change_count: 41,
                items,
            }]),
            reads: reads.clone(),
        },
        reads,
    )
}
fn fail_insert(db: &rusqlite::Connection) {
    db.execute_batch("CREATE TRIGGER synthetic_insert_failure BEFORE INSERT ON clips BEGIN SELECT RAISE(ABORT, 'synthetic insert failure'); END;").expect("fault trigger");
}

#[test]
fn transient_write_failure_retries_original_snapshot_without_reading_a_new_clipboard() {
    let root = tempfile::tempdir().expect("private test dir");
    let path = root.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let originals = vec![
        item(&device, "first original", 3),
        item(&device, "second original", 1),
    ];
    let (source, reads) = source(originals.clone());
    let mut coordinator = CaptureCoordinator::new(source, store.clone(), device);
    let db = rusqlite::Connection::open(&path).expect("fault connection");
    fail_insert(&db);
    assert!(coordinator.tick(Utc::now()).is_err());
    assert!(
        store
            .list_history(SearchPage::default())
            .expect("history")
            .is_empty()
    );
    assert_eq!(store.blob_count().expect("no partial blobs"), 0);
    db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
        .expect("recover storage");
    let event = coordinator.tick(Utc::now()).expect("retry");
    let CaptureEvent::Stored {
        change_count,
        clip_ids,
    } = event
    else {
        panic!("failed capture must be retried, not forgotten: {event:?}");
    };
    assert_eq!(change_count, 41);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "retry the owned original, not whatever is now on the clipboard"
    );
    assert_eq!(clip_ids.len(), 2);
    for (id, original) in clip_ids.iter().zip(&originals) {
        assert_eq!(
            store.load_clipboard_payload(*id).expect("original bytes"),
            original.representations
        );
        assert_eq!(
            store
                .get_clip(*id)
                .expect("saved clip")
                .expect("clip")
                .captured_at
                .timestamp_millis(),
            original.captured_at.timestamp_millis()
        );
    }
    assert_eq!(
        coordinator.tick(Utc::now()).expect("idle after recovery"),
        CaptureEvent::Unchanged
    );
    drop(coordinator);
    drop(store);
    drop(db);
    assert_eq!(
        SqliteStore::open(&path)
            .expect("restart")
            .list_history(SearchPage::default())
            .expect("durable history")
            .len(),
        2
    );
}

#[test]
fn ongoing_storage_failure_does_not_turn_into_an_unchanged_success() {
    let root = tempfile::tempdir().expect("private test dir");
    let path = root.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let (source, reads) = source(vec![item(&device, "pending original", 0)]);
    let mut coordinator = CaptureCoordinator::new(source, store, device);
    let db = rusqlite::Connection::open(path).expect("fault connection");
    fail_insert(&db);
    for _ in 0..3 {
        assert!(
            coordinator.tick(Utc::now()).is_err(),
            "unresolved storage failure must remain visible"
        );
    }
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn retention_failure_rolls_back_capture_index_blobs_and_outbox_before_retry() {
    let root = tempfile::tempdir().expect("private test dir");
    let path = root.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let old = store
        .insert_capture(&item(&device, "older kept until commit", 60))
        .expect("old capture");
    let before = store
        .prepare_private_sync_batch(250)
        .expect("before outbox");
    let (source, _) = source(vec![item(&device, "new capture", 0)]);
    let mut coordinator = CaptureCoordinator::new(source, store.clone(), device);
    coordinator
        .set_retention(RetentionPolicy {
            max_age_days: None,
            max_unpinned_items: Some(1),
        })
        .expect("retention");
    let db = rusqlite::Connection::open(path).expect("fault connection");
    db.execute_batch("CREATE TRIGGER synthetic_retention_failure BEFORE DELETE ON clips BEGIN SELECT RAISE(ABORT, 'synthetic retention failure'); END;").expect("retention fault");
    assert!(coordinator.tick(Utc::now()).is_err());
    assert_eq!(
        store.list_history(SearchPage::default()).expect("history"),
        vec![old.clone()],
        "capture and retention must commit together"
    );
    assert_eq!(store.blob_count().expect("unchanged blobs"), 1);
    assert_eq!(
        store
            .prepare_private_sync_batch(250)
            .expect("unchanged outbox")
            .operations,
        before.operations
    );
    let documents: i64 = db
        .query_row("SELECT COUNT(*) FROM clip_search_documents", [], |row| {
            row.get(0)
        })
        .expect("index count");
    assert_eq!(documents, 1);
    db.execute_batch("DROP TRIGGER synthetic_retention_failure;")
        .expect("recover cleanup");
    assert!(matches!(
        coordinator.tick(Utc::now()).expect("retry"),
        CaptureEvent::Stored {
            change_count: 41,
            ..
        }
    ));
    let history = store.list_history(SearchPage::default()).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].title, "new capture");
    assert!(store.get_clip(old.id).expect("old removed").is_none());
}

fn blocked_capture() -> (
    tempfile::TempDir,
    rusqlite::Connection,
    CaptureCoordinator<Source>,
    Arc<SqliteStore>,
) {
    let root = tempfile::tempdir().expect("private test dir");
    let path = root.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let (source, _) = source(vec![item(&device, "pending original", 0)]);
    let mut coordinator = CaptureCoordinator::new(source, store.clone(), device);
    let db = rusqlite::Connection::open(path).expect("fault connection");
    fail_insert(&db);
    assert!(coordinator.tick(Utc::now()).is_err());
    assert_eq!(coordinator.pending_item_count(), 1);
    (root, db, coordinator, store)
}

#[test]
fn pause_discards_uncommitted_memory_even_if_resumed_before_the_next_tick() {
    for timed in [false, true] {
        let (_root, db, mut coordinator, store) = blocked_capture();
        if timed {
            coordinator.pause_for(Utc::now(), Duration::minutes(5));
        } else {
            coordinator.pause_indefinitely();
        }
        assert_eq!(coordinator.pending_item_count(), 0);
        coordinator.resume().expect("resume boundary");
        db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
            .expect("recover storage");
        assert!(matches!(
            coordinator.tick(Utc::now()).expect("discard notice"),
            CaptureEvent::DiscardedWhilePaused {
                change_count: 41,
                item_count: 1
            }
        ));
        assert_eq!(
            coordinator.tick(Utc::now()).expect("idle"),
            CaptureEvent::Unchanged
        );
        assert!(
            store
                .list_history(SearchPage::default())
                .expect("no delayed capture")
                .is_empty()
        );
    }
}

#[test]
fn checkpoint_applies_pause_or_policy_before_retrying_pending_storage() {
    for privacy in [false, true] {
        let (_root, db, mut coordinator, store) = blocked_capture();
        db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
            .expect("recover storage");
        let event = coordinator
            .tick_with_checkpoint(Utc::now(), |coordinator| {
                if privacy {
                    coordinator
                        .privacy_policy_mut()
                        .exclude_bundle_id("com.synthetic.Editor");
                    Some(CaptureInterruption::Privacy)
                } else {
                    coordinator.pause_indefinitely();
                    coordinator.resume().expect("resume boundary");
                    Some(CaptureInterruption::Pause)
                }
            })
            .expect("control wins before pending retry");
        assert!(matches!(
            event,
            CaptureEvent::DiscardedWhilePaused { .. }
                | CaptureEvent::DiscardedForPrivacyChange { .. }
        ));
        assert_eq!(coordinator.pending_item_count(), 0);
        assert!(
            store
                .list_history(SearchPage::default())
                .expect("no delayed insert")
                .is_empty()
        );
        assert_eq!(store.blob_count().expect("no payload blobs"), 0);
    }
}

#[test]
fn privacy_changes_invalidate_pending_memory_even_if_settings_are_changed_back() {
    for direct_mutation in [false, true] {
        let (_root, db, mut coordinator, store) = blocked_capture();
        if direct_mutation {
            coordinator
                .privacy_policy_mut()
                .exclude_bundle_id("com.synthetic.Editor");
        } else {
            coordinator
                .set_preferences(paste_domain::CapturePreferences {
                    excluded_bundle_ids: vec!["com.synthetic.Editor".into()],
                    ..Default::default()
                })
                .expect("exclude");
            coordinator
                .set_preferences(paste_domain::CapturePreferences::default())
                .expect("revert settings");
        }
        assert_eq!(coordinator.pending_item_count(), 0);
        db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
            .expect("recover storage");
        assert!(matches!(
            coordinator.tick(Utc::now()).expect("privacy discard"),
            CaptureEvent::DiscardedForPrivacyChange {
                change_count: 41,
                item_count: 1
            }
        ));
        assert!(
            store
                .list_history(SearchPage::default())
                .expect("no delayed private capture")
                .is_empty()
        );
    }
}

#[test]
fn retained_batches_obey_limits_and_invalid_payloads_do_not_wedge_future_capture() {
    let (_root, db, mut coordinator, store) = blocked_capture();
    coordinator.set_limits(CaptureLimits {
        max_total_bytes: 2,
        ..Default::default()
    });
    db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
        .expect("recover storage");
    assert!(
        coordinator.tick(Utc::now()).is_err(),
        "lower resource bound discards cached bytes"
    );
    assert_eq!(coordinator.pending_item_count(), 0);
    assert!(
        store
            .list_history(SearchPage::default())
            .expect("bounded cache")
            .is_empty()
    );

    for limits in [
        CaptureLimits {
            max_items: 0,
            ..Default::default()
        },
        CaptureLimits {
            max_representations_per_item: 0,
            ..Default::default()
        },
        CaptureLimits {
            max_representation_bytes: 2,
            ..Default::default()
        },
        CaptureLimits {
            max_total_bytes: 2,
            ..Default::default()
        },
    ] {
        let store = Arc::new(SqliteStore::open_in_memory().expect("store"));
        let device = store.get_or_create_device("Synthetic Mac").expect("device");
        let (mut source, _) = source(vec![item(&device, "oversized", 0)]);
        source.polls.push_back(ClipboardPoll::Captured {
            change_count: 42,
            items: vec![item(&device, "ok", 0)],
        });
        let mut coordinator = CaptureCoordinator::new(source, store.clone(), device);
        coordinator.set_limits(limits);
        assert!(coordinator.tick(Utc::now()).is_err());
        assert_eq!(coordinator.pending_item_count(), 0);
        coordinator.set_limits(CaptureLimits::default());
        assert!(matches!(
            coordinator.tick(Utc::now()).expect("next valid copy"),
            CaptureEvent::Stored {
                change_count: 42,
                ..
            }
        ));
        assert_eq!(
            store
                .list_history(SearchPage::default())
                .expect("only valid capture")
                .len(),
            1
        );
    }
}

#[test]
fn recovery_persists_pending_then_polls_the_next_available_generation_once() {
    let root = tempfile::tempdir().expect("private test dir");
    let path = root.path().join("history.db");
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let (mut source, reads) = source(vec![item(&device, "pending", 1)]);
    source.polls.push_back(ClipboardPoll::Captured {
        change_count: 42,
        items: vec![item(&device, "next available", 0)],
    });
    let mut coordinator = CaptureCoordinator::new(source, store.clone(), device);
    let db = rusqlite::Connection::open(path).expect("fault connection");
    fail_insert(&db);
    assert!(coordinator.tick(Utc::now()).is_err());
    db.execute_batch("DROP TRIGGER synthetic_insert_failure;")
        .expect("recover storage");
    assert!(matches!(
        coordinator.tick(Utc::now()).expect("retry original"),
        CaptureEvent::Stored {
            change_count: 41,
            ..
        }
    ));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(matches!(
        coordinator
            .tick(Utc::now())
            .expect("next available generation"),
        CaptureEvent::Stored {
            change_count: 42,
            ..
        }
    ));
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        store
            .list_history(SearchPage::default())
            .expect("two captures")
            .len(),
        2
    );
}
