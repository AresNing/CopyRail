use std::{collections::VecDeque, sync::Arc};

use chrono::{Duration, Utc};
use paste_core::{CaptureCoordinator, CaptureEvent, PauseState};
use paste_domain::{
    CaptureFlags, CapturePreferences, CapturedItem, CapturedRepresentation, DeviceId,
    DeviceMetadata, RetentionPolicy, SearchPage, SourceApplication,
};
use paste_platform::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
    IgnoreReason,
};
use paste_storage::SqliteStore;

struct FakeClipboard {
    polls: VecDeque<ClipboardPoll>,
}

impl ClipboardSource for FakeClipboard {
    fn discard_current(&mut self) -> Result<(), ClipboardError> {
        self.polls.clear();
        Ok(())
    }

    fn poll(
        &mut self,
        _policy: &ClipboardPrivacyPolicy,
        _limits: CaptureLimits,
        _device: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        Ok(self.polls.pop_front().unwrap_or(ClipboardPoll::Unchanged))
    }
}

fn device() -> DeviceMetadata {
    DeviceMetadata {
        id: DeviceId::from_uuid(uuid::Uuid::nil()),
        display_name: "Test Mac".into(),
    }
}

fn captured(text: &str) -> CapturedItem {
    CapturedItem {
        captured_at: Utc::now(),
        source: SourceApplication {
            bundle_identifier: "com.example.Editor".into(),
            display_name: "Editor".into(),
        },
        device: device(),
        flags: CaptureFlags::default(),
        representations: vec![CapturedRepresentation::plain_text(text)],
    }
}

#[test]
fn stores_a_captured_batch_atomically() {
    let source = FakeClipboard {
        polls: VecDeque::from([ClipboardPoll::Captured {
            change_count: 9,
            items: vec![captured("one"), captured("two")],
        }]),
    };
    let store = SqliteStore::open_in_memory().expect("database");
    let mut coordinator = CaptureCoordinator::new(source, Arc::new(store), device());

    let event = coordinator.tick(Utc::now()).expect("capture tick");
    let CaptureEvent::Stored { clip_ids, .. } = event else {
        panic!("expected stored event");
    };
    assert_eq!(clip_ids.len(), 2);
    assert_eq!(
        coordinator
            .store()
            .list_history(SearchPage::default())
            .expect("history")
            .len(),
        2
    );
}

#[test]
fn pause_discards_changes_instead_of_capturing_them_after_resume() {
    let source = FakeClipboard {
        polls: VecDeque::from([
            ClipboardPoll::Captured {
                change_count: 10,
                items: vec![captured("do not keep")],
            },
            ClipboardPoll::Unchanged,
        ]),
    };
    let store = SqliteStore::open_in_memory().expect("database");
    let mut coordinator = CaptureCoordinator::new(source, Arc::new(store), device());
    let now = Utc::now();
    coordinator.pause_for(now, Duration::minutes(5));

    assert!(matches!(
        coordinator.tick(now).expect("paused tick"),
        CaptureEvent::DiscardedWhilePaused { item_count: 1, .. }
    ));
    coordinator.resume().expect("resume boundary");
    assert_eq!(coordinator.pause_state(), PauseState::Running);
    assert_eq!(
        coordinator.tick(now).expect("resumed tick"),
        CaptureEvent::Unchanged
    );
    assert!(
        coordinator
            .store()
            .list_history(SearchPage::default())
            .expect("history")
            .is_empty()
    );
}

#[test]
fn surfaces_privacy_ignores_without_writing() {
    let source = FakeClipboard {
        polls: VecDeque::from([ClipboardPoll::Ignored {
            change_count: 11,
            reason: IgnoreReason::Confidential,
        }]),
    };
    let store = SqliteStore::open_in_memory().expect("database");
    let mut coordinator = CaptureCoordinator::new(source, Arc::new(store), device());

    assert!(matches!(
        coordinator.tick(Utc::now()).expect("tick"),
        CaptureEvent::Ignored {
            reason: IgnoreReason::Confidential,
            ..
        }
    ));
}

#[test]
fn applies_the_configured_retention_policy_after_capture() {
    let source = FakeClipboard {
        polls: VecDeque::from([ClipboardPoll::Captured {
            change_count: 12,
            items: vec![captured("older"), captured("newer")],
        }]),
    };
    let store = SqliteStore::open_in_memory().expect("database");
    let mut coordinator = CaptureCoordinator::new(source, Arc::new(store), device());
    coordinator
        .set_preferences(CapturePreferences {
            retention: RetentionPolicy {
                max_age_days: None,
                max_unpinned_items: Some(1),
            },
            excluded_bundle_ids: vec!["com.example.passwords".into()],
        })
        .expect("preferences");

    coordinator.tick(Utc::now()).expect("capture tick");
    assert_eq!(
        coordinator
            .store()
            .list_history(SearchPage::default())
            .expect("history")
            .len(),
        1
    );
}
