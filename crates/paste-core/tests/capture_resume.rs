use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use paste_core::{CaptureCoordinator, CaptureEvent, CaptureInterruption, PauseState};
use paste_domain::{CapturedItem, CapturedRepresentation, DeviceMetadata, SearchPage};
use paste_platform::{
    CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy, ClipboardSource,
};
use paste_storage::SqliteStore;

// Model the current generation, not a queue of already captured snapshots:
// a copy can arrive after the last paused poll and before resume is processed.
#[derive(Default)]
struct State {
    generation: i64,
    payload_reads: usize,
    boundary_reads: usize,
    fail_boundary: bool,
}

struct Source {
    state: Arc<Mutex<State>>,
    consumed: i64,
}

impl ClipboardSource for Source {
    fn discard_current(&mut self) -> Result<(), ClipboardError> {
        let mut state = self.state.lock().expect("synthetic boundary");
        state.boundary_reads += 1;
        if state.fail_boundary {
            return Err(ClipboardError::Unavailable);
        }
        self.consumed = state.generation;
        Ok(())
    }

    fn poll(
        &mut self,
        _: &ClipboardPrivacyPolicy,
        _: CaptureLimits,
        device: &DeviceMetadata,
    ) -> Result<ClipboardPoll, ClipboardError> {
        let mut state = self.state.lock().expect("synthetic state");
        if state.generation == self.consumed {
            return Ok(ClipboardPoll::Unchanged);
        }
        self.consumed = state.generation;
        state.payload_reads += 1;
        Ok(ClipboardPoll::Captured {
            change_count: self.consumed,
            items: vec![CapturedItem {
                captured_at: Utc::now(),
                source: paste_domain::SourceApplication::unknown(),
                device: device.clone(),
                flags: Default::default(),
                representations: vec![CapturedRepresentation::plain_text(format!(
                    "synthetic generation {}",
                    self.consumed
                ))],
            }],
        })
    }
}

fn coordinator() -> (CaptureCoordinator<Source>, Arc<Mutex<State>>) {
    let store = Arc::new(SqliteStore::open_in_memory().expect("synthetic store"));
    let device = store.get_or_create_device("Synthetic Mac").expect("device");
    let state = Arc::new(Mutex::new(State::default()));
    (
        CaptureCoordinator::new(
            Source {
                state: state.clone(),
                consumed: 0,
            },
            store,
            device,
        ),
        state,
    )
}

fn assert_no_history(coordinator: &CaptureCoordinator<Source>) {
    assert!(
        coordinator
            .store()
            .list_history(SearchPage::default())
            .expect("history")
            .is_empty(),
        "a paused generation must not enter persistent history"
    );
    assert_eq!(coordinator.store().blob_count().expect("blobs"), 0);
}

#[test]
fn manual_resume_skips_copy_after_last_paused_poll_but_keeps_future_copy() {
    let (mut coordinator, state) = coordinator();
    let now = Utc::now();
    coordinator.pause_indefinitely();
    assert_eq!(
        coordinator.tick(now).expect("paused idle"),
        CaptureEvent::Unchanged
    );
    state.lock().expect("copy while paused").generation = 1;
    coordinator.resume().expect("resume boundary");
    assert_eq!(coordinator.pause_state(), PauseState::Running);
    assert_eq!(
        coordinator.tick(now).expect("resume tick"),
        CaptureEvent::Unchanged
    );
    assert_no_history(&coordinator);
    assert_eq!(state.lock().expect("read count").payload_reads, 0);
    state.lock().expect("new copy after resume").generation = 2;
    assert!(matches!(
        coordinator.tick(now).expect("future copy"),
        CaptureEvent::Stored {
            change_count: 2,
            ..
        }
    ));
}

#[test]
fn timed_resume_skips_unobserved_paused_copy_at_the_deadline() {
    let (mut coordinator, state) = coordinator();
    let now = Utc::now();
    coordinator.pause_for(now, Duration::seconds(10));
    coordinator.tick(now).expect("last paused poll");
    state.lock().expect("copy just before expiry").generation = 1;
    assert_eq!(
        coordinator
            .tick(now + Duration::seconds(10))
            .expect("expiry"),
        CaptureEvent::Unchanged
    );
    assert_eq!(coordinator.pause_state(), PauseState::Running);
    assert_no_history(&coordinator);
    assert_eq!(state.lock().expect("read count").payload_reads, 0);
    state
        .lock()
        .expect("copy after confirmed expiry")
        .generation = 2;
    assert!(matches!(
        coordinator
            .tick(now + Duration::seconds(11))
            .expect("future copy"),
        CaptureEvent::Stored {
            change_count: 2,
            ..
        }
    ));
}

#[test]
fn repeated_resume_while_running_does_not_skip_a_new_copy() {
    let (mut coordinator, state) = coordinator();
    state.lock().expect("running copy").generation = 1;
    coordinator.resume().expect("idempotent resume");
    coordinator.resume().expect("duplicate resume");
    assert_eq!(state.lock().expect("no boundary").boundary_reads, 0);
    assert!(matches!(
        coordinator.tick(Utc::now()).expect("running copy"),
        CaptureEvent::Stored {
            change_count: 1,
            ..
        }
    ));
}

#[test]
fn failed_manual_boundary_keeps_pause_until_a_successful_retry() {
    let (mut coordinator, state) = coordinator();
    coordinator.pause_indefinitely();
    {
        let mut state = state.lock().expect("paused copy");
        state.generation = 1;
        state.fail_boundary = true;
    }
    assert!(coordinator.resume().is_err());
    assert_eq!(coordinator.pause_state(), PauseState::Indefinite);
    assert_no_history(&coordinator);
    assert_eq!(state.lock().expect("no payload read").payload_reads, 0);
    {
        let mut state = state.lock().expect("retry boundary");
        state.generation = 2;
        state.fail_boundary = false;
    }
    coordinator.resume().expect("confirmed retry");
    assert_eq!(
        coordinator.tick(Utc::now()).expect("no replay"),
        CaptureEvent::Unchanged
    );
    assert_eq!(state.lock().expect("boundary calls").boundary_reads, 2);
    assert_no_history(&coordinator);
    state.lock().expect("new running copy").generation = 3;
    assert!(matches!(
        coordinator.tick(Utc::now()).expect("new copy"),
        CaptureEvent::Stored {
            change_count: 3,
            ..
        }
    ));
}

#[test]
fn failed_timed_boundary_preserves_pause_and_controls_without_reading_payload() {
    let (mut coordinator, state) = coordinator();
    let now = Utc::now();
    let deadline = now + Duration::seconds(10);
    coordinator.pause_for(now, Duration::seconds(10));
    coordinator
        .tick(deadline - Duration::milliseconds(1))
        .expect("not expired");
    assert_eq!(state.lock().expect("not attempted early").boundary_reads, 0);
    {
        let mut state = state.lock().expect("unobserved paused copy");
        state.generation = 1;
        state.fail_boundary = true;
    }
    let mut checkpoint_ran = false;
    assert!(
        coordinator
            .tick_with_checkpoint(deadline, |_| {
                checkpoint_ran = true;
                None
            })
            .is_err()
    );
    assert!(
        checkpoint_ran,
        "failed reset must still let queued controls run"
    );
    assert_eq!(coordinator.pause_state(), PauseState::Until(deadline));
    assert!(coordinator.tick(deadline + Duration::seconds(1)).is_err());
    assert_no_history(&coordinator);
    assert_eq!(state.lock().expect("no payload read").payload_reads, 0);
    {
        let mut state = state.lock().expect("provider recovered");
        state.generation = 2;
        state.fail_boundary = false;
    }
    assert_eq!(
        coordinator
            .tick(deadline + Duration::seconds(2))
            .expect("retry expiry"),
        CaptureEvent::Unchanged
    );
    assert_eq!(coordinator.pause_state(), PauseState::Running);
    assert_no_history(&coordinator);
    assert_eq!(
        state
            .lock()
            .expect("one boundary per attempt")
            .boundary_reads,
        3
    );
}

#[test]
fn resume_at_read_checkpoint_skips_the_replacement_and_discards_in_flight_content() {
    let (mut coordinator, state) = coordinator();
    state.lock().expect("initial running copy").generation = 1;
    let event = coordinator
        .tick_with_checkpoint(Utc::now(), |coordinator| {
            coordinator.pause_indefinitely();
            state.lock().expect("paused replacement").generation = 2;
            coordinator.resume().expect("checkpoint resume");
            Some(CaptureInterruption::Pause)
        })
        .expect("interrupted read");
    assert!(matches!(
        event,
        CaptureEvent::DiscardedWhilePaused {
            change_count: 1,
            ..
        }
    ));
    assert_eq!(
        coordinator.tick(Utc::now()).expect("replacement skipped"),
        CaptureEvent::Unchanged
    );
    assert_no_history(&coordinator);
    state.lock().expect("future copy").generation = 3;
    assert!(matches!(
        coordinator.tick(Utc::now()).expect("new running copy"),
        CaptureEvent::Stored {
            change_count: 3,
            ..
        }
    ));
}
