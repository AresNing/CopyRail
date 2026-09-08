use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration as StdDuration, Instant},
};

use chrono::{Duration, Utc};
use paste_core::{CaptureCoordinator, CaptureEvent, CaptureInterruption, CoreError, PauseState};
use paste_domain::{CapturePreferences, DeviceMetadata};
use paste_platform::{ClipboardSource, MacClipboardReader};
use paste_storage::SqliteStore;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    #[serde(default)]
    pub isolated: bool,
    pub paused: bool,
    pub paused_until_ms: Option<i64>,
    pub last_error: Option<String>,
    pub last_change_count: Option<i64>,
    #[serde(default)]
    pub pending_items: usize,
    #[serde(default)]
    pub control_pending: Option<CaptureControlKind>,
    #[serde(default)]
    pub revision: u64,
    #[serde(skip)]
    request_id: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureControlKind {
    Pause,
    Resume,
    Preferences,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryChanged {
    clip_ids: Vec<String>,
}

enum CaptureControl {
    PauseFor(Duration),
    PauseIndefinitely,
    Resume,
    UpdatePreferences(CapturePreferences),
    Stop,
}

impl CaptureControl {
    fn kind(&self) -> Option<CaptureControlKind> {
        match self {
            Self::PauseFor(_) | Self::PauseIndefinitely => Some(CaptureControlKind::Pause),
            Self::Resume => Some(CaptureControlKind::Resume),
            Self::UpdatePreferences(_) => Some(CaptureControlKind::Preferences),
            Self::Stop => None,
        }
    }
}

struct ControlMessage {
    id: u64,
    control: CaptureControl,
}

pub struct CaptureService {
    sender: Option<mpsc::Sender<ControlMessage>>,
    status: Arc<Mutex<CaptureStatus>>,
}

impl CaptureService {
    pub fn isolated() -> Self {
        Self {
            sender: None,
            status: Arc::new(Mutex::new(CaptureStatus {
                paused: true,
                isolated: true,
                ..Default::default()
            })),
        }
    }

    fn send(&self, control: CaptureControl) -> Result<CaptureStatus, String> {
        let sender = self
            .sender
            .as_ref()
            .ok_or("隔离验证不会启动系统剪贴板采集。")?;
        // Publish intent and queue order together. The worker cannot confirm a
        // command and then have this caller overwrite it with a stale pending
        // flag. This lock never covers platform reads or SQLite operations.
        let mut status = self
            .status
            .lock()
            .map_err(|_| "capture status lock was poisoned")?;
        let id = status
            .request_id
            .checked_add(1)
            .ok_or("capture request sequence exhausted")?;
        let revision = status
            .revision
            .checked_add(1)
            .ok_or("capture status sequence exhausted")?;
        let kind = control.kind();
        sender
            .send(ControlMessage { id, control })
            .map_err(|error| error.to_string())?;
        status.request_id = id;
        status.control_pending = kind;
        status.revision = revision;
        Ok(status.clone())
    }
    pub fn spawn(
        app: AppHandle,
        store: Arc<SqliteStore>,
        device: DeviceMetadata,
        preferences: CapturePreferences,
    ) -> Result<Self, String> {
        Self::spawn_with_source(
            store,
            device,
            preferences,
            || MacClipboardReader::new(false),
            move |clip_ids| {
                let _ = app.emit(
                    "history-changed",
                    HistoryChanged {
                        clip_ids: clip_ids
                            .into_iter()
                            .map(|id: paste_domain::ClipId| id.to_string())
                            .collect(),
                    },
                );
            },
        )
    }

    fn spawn_with_source<S: ClipboardSource + 'static>(
        store: Arc<SqliteStore>,
        device: DeviceMetadata,
        preferences: CapturePreferences,
        source: impl FnOnce() -> S + Send + 'static,
        mut stored: impl FnMut(Vec<paste_domain::ClipId>) + Send + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(CaptureStatus::default()));
        let thread_status = Arc::clone(&status);
        thread::Builder::new()
            .name("pasters-clipboard-capture".into())
            .spawn(move || {
                let reader = source();
                let mut coordinator = CaptureCoordinator::new(reader, store, device);
                let mut retry = CaptureRetry::default();
                if let Err(error) = coordinator.set_preferences(preferences) {
                    update_status(&thread_status, |value| {
                        value.last_error = Some(error.to_string());
                    });
                }
                loop {
                    let mut stop = false;
                    process_controls(&receiver, &mut coordinator, &thread_status, &mut stop);
                    if stop {
                        break;
                    }

                    if !retry.ready(Instant::now(), coordinator.pending_item_count() > 0) {
                        // Continue checking pause/privacy/stop controls on the
                        // normal cadence while storage retries back off.
                        thread::sleep(StdDuration::from_millis(125));
                        continue;
                    }

                    let result = coordinator.tick_with_checkpoint(Utc::now(), |coordinator| {
                        process_controls(&receiver, coordinator, &thread_status, &mut stop)
                    });
                    retry.observe(
                        Instant::now(),
                        result.is_err() && coordinator.pending_item_count() > 0,
                    );
                    update_status(&thread_status, |value| {
                        record_tick(
                            value,
                            &result,
                            coordinator.pause_state(),
                            coordinator.pending_item_count(),
                        );
                    });
                    if let Ok(CaptureEvent::Stored { clip_ids, .. }) = result {
                        stored(clip_ids);
                    }
                    if stop {
                        break;
                    }
                    thread::sleep(StdDuration::from_millis(125));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender: Some(sender),
            status,
        })
    }

    pub fn pause(&self, minutes: Option<u32>) -> Result<CaptureStatus, String> {
        if minutes == Some(0) {
            return Err("pause duration must be positive".into());
        }
        let duration = minutes.map(|value| Duration::minutes(i64::from(value)));
        let control = duration.map_or(CaptureControl::PauseIndefinitely, CaptureControl::PauseFor);
        self.send(control)
    }

    pub fn resume(&self) -> Result<CaptureStatus, String> {
        self.send(CaptureControl::Resume)
    }

    pub fn status(&self) -> Result<CaptureStatus, String> {
        self.status
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "capture status lock was poisoned".into())
    }

    pub fn update_preferences(&self, preferences: CapturePreferences) -> Result<(), String> {
        self.send(CaptureControl::UpdatePreferences(preferences))
            .map(|_| ())
    }
}

impl Drop for CaptureService {
    fn drop(&mut self) {
        let _ = self.send(CaptureControl::Stop);
    }
}

fn update_status(status: &Mutex<CaptureStatus>, update: impl FnOnce(&mut CaptureStatus)) {
    if let Ok(mut value) = status.lock() {
        let previous = value.clone();
        update(&mut value);
        if *value != previous {
            value.revision = previous.revision.saturating_add(1);
        }
    }
}

fn process_controls<S: ClipboardSource>(
    receiver: &mpsc::Receiver<ControlMessage>,
    coordinator: &mut CaptureCoordinator<S>,
    status: &Mutex<CaptureStatus>,
    stopped: &mut bool,
) -> Option<CaptureInterruption> {
    let mut interruption = None;
    // Bound work in case menu/IPC requests are continuously queued. Unhandled
    // requests remain visibly pending; they are never acknowledged early.
    for _ in 0..64 {
        let Ok(message) = receiver.try_recv() else {
            break;
        };
        let mut error = None;
        match message.control {
            CaptureControl::PauseFor(duration) => {
                coordinator.pause_for(Utc::now(), duration);
                interruption = Some(CaptureInterruption::Pause);
            }
            CaptureControl::PauseIndefinitely => {
                coordinator.pause_indefinitely();
                interruption = Some(CaptureInterruption::Pause);
            }
            CaptureControl::Resume => {
                if let Err(failure) = coordinator.resume() {
                    error = Some(format!("最近一次恢复采集失败，该次未解除暂停：{failure}"));
                    interruption = Some(CaptureInterruption::Pause);
                }
            }
            CaptureControl::UpdatePreferences(preferences) => {
                let previous = coordinator.privacy_policy().clone();
                if let Err(failure) = coordinator.set_preferences(preferences) {
                    error = Some(failure.to_string());
                    interruption = Some(CaptureInterruption::Privacy);
                } else if &previous != coordinator.privacy_policy() {
                    interruption = Some(CaptureInterruption::Privacy);
                }
            }
            CaptureControl::Stop => {
                coordinator.pause_indefinitely();
                interruption = Some(CaptureInterruption::Pause);
                *stopped = true;
            }
        }
        update_status(status, |value| {
            value.paused = coordinator.pause_state() != PauseState::Running;
            value.paused_until_ms = match coordinator.pause_state() {
                PauseState::Until(until) => Some(until.timestamp_millis()),
                _ => None,
            };
            value.pending_items = coordinator.pending_item_count();
            if value.request_id == message.id {
                value.control_pending = None;
            }
            if error.is_some() {
                value.last_error = error;
            }
        });
        if *stopped {
            break;
        }
    }
    interruption
}

#[derive(Default)]
struct CaptureRetry {
    delay_ms: u64,
    next: Option<Instant>,
}

impl CaptureRetry {
    fn ready(&self, now: Instant, pending: bool) -> bool {
        !pending || self.next.is_none_or(|next| now >= next)
    }

    fn observe(&mut self, now: Instant, pending_failure: bool) {
        if pending_failure {
            self.delay_ms = if self.delay_ms == 0 {
                250
            } else {
                (self.delay_ms * 2).min(5_000)
            };
            self.next = Some(now + StdDuration::from_millis(self.delay_ms));
        } else {
            self.delay_ms = 0;
            self.next = None;
        }
    }
}

fn record_tick(
    status: &mut CaptureStatus,
    result: &Result<CaptureEvent, CoreError>,
    pause: PauseState,
    pending_items: usize,
) {
    status.pending_items = pending_items;
    status.paused = pause != PauseState::Running;
    status.paused_until_ms = if let PauseState::Until(until) = pause {
        Some(until.timestamp_millis())
    } else {
        None
    };
    match result {
        Ok(CaptureEvent::Stored { change_count, .. }) => {
            status.last_change_count = Some(*change_count);
            status.last_error = None;
        }
        Ok(CaptureEvent::DiscardedWhilePaused { change_count, .. }) => {
            status.last_change_count = Some(*change_count);
            status.last_error = None;
        }
        Ok(CaptureEvent::DiscardedForPrivacyChange {
            change_count,
            item_count,
        }) => {
            status.last_change_count = Some(*change_count);
            status.last_error = Some(format!(
                "隐私设置已变更，已丢弃 {item_count} 条未写入缓存，不会补录。"
            ));
        }
        Ok(CaptureEvent::Ignored { change_count, .. }) => {
            status.last_change_count = Some(*change_count);
            // An intentionally ignored copy does not prove storage recovered.
        }
        Ok(CaptureEvent::Unchanged) => {
            // Keep the error visible; an idle clipboard is not a successful save.
        }
        Err(CoreError::Clipboard(paste_platform::ClipboardError::ChangedDuringRead)) => {
            // Another copy superseded this in-flight read. No coherent snapshot
            // was handed to storage. Retry on the next normal tick without a
            // misleading storage failure or clearing an earlier real error.
        }
        Err(error) => {
            status.last_error = Some(if pending_items > 0 {
                format!(
                    "采集保存失败，{pending_items} 条内容暂存在内存等待重试；退出会丢失这些缓存，故障期间的新复制可能遗漏。原因：{error}"
                )
            } else {
                format!("最近一次采集未成功：{error}")
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked_capture() -> (
        CaptureService,
        Arc<SqliteStore>,
        mpsc::Sender<()>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use paste_domain::{CaptureFlags, CapturedItem, CapturedRepresentation, SourceApplication};
        use paste_platform::{
            CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Source {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
            reads: Arc<AtomicUsize>,
        }
        impl ClipboardSource for Source {
            fn discard_current(&mut self) -> Result<(), ClipboardError> {
                // The single synthetic generation has already been read.
                Ok(())
            }

            fn poll(
                &mut self,
                _: &ClipboardPrivacyPolicy,
                _: CaptureLimits,
                device: &DeviceMetadata,
            ) -> Result<ClipboardPoll, ClipboardError> {
                if self.reads.fetch_add(1, Ordering::SeqCst) > 0 {
                    return Ok(ClipboardPoll::Unchanged);
                }
                self.entered
                    .send(())
                    .expect("test receives source checkpoint");
                // Finite timeout/disconnection prevents a failed assertion from
                // leaving a permanently blocked test worker behind.
                self.release
                    .recv_timeout(StdDuration::from_secs(5))
                    .expect("release synthetic read");
                Ok(ClipboardPoll::Captured {
                    change_count: 7,
                    items: vec![CapturedItem {
                        captured_at: Utc::now(),
                        source: SourceApplication::unknown(),
                        device: device.clone(),
                        flags: CaptureFlags::default(),
                        representations: vec![CapturedRepresentation::plain_text(
                            "Synthetic in-flight copy",
                        )],
                    }],
                })
            }
        }
        let store = Arc::new(SqliteStore::open_in_memory().expect("synthetic history"));
        let device = store.get_or_create_device("Synthetic Mac").expect("device");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let reads = Arc::new(AtomicUsize::new(0));
        let source_reads = Arc::clone(&reads);
        let service = CaptureService::spawn_with_source(
            Arc::clone(&store),
            device,
            CapturePreferences::default(),
            move || Source {
                entered: entered_tx,
                release: release_rx,
                reads: source_reads,
            },
            |_| {},
        )
        .expect("synthetic capture worker");
        entered_rx
            .recv_timeout(StdDuration::from_secs(3))
            .expect("reader blocked");
        (service, store, release_tx, reads)
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + StdDuration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "worker checkpoint timed out");
            thread::sleep(StdDuration::from_millis(5));
        }
    }

    #[test]
    fn pause_does_not_claim_confirmation_while_source_is_blocked() {
        let (service, _, release, _) = blocked_capture();
        let response = service.pause(None).expect("request pause");
        release.send(()).expect("release read before assertions");
        assert!(!response.paused, "pause is not yet confirmed by the worker");
        assert_eq!(response.control_pending, Some(CaptureControlKind::Pause));
        wait_until(|| service.status().expect("status").paused);
    }

    #[test]
    fn pause_queued_during_read_discards_the_in_flight_copy_before_storage() {
        use std::sync::atomic::Ordering;
        let (service, store, release, reads) = blocked_capture();
        service.pause(None).expect("request pause");
        release.send(()).expect("finish synthetic read");
        wait_until(|| reads.load(Ordering::SeqCst) >= 2);
        assert!(service.status().expect("status").paused);
        assert!(
            store
                .list_history(paste_domain::SearchPage::default())
                .expect("history")
                .is_empty()
        );
    }

    #[test]
    fn rapid_pause_resume_and_privacy_reversion_cannot_revalidate_in_flight_content() {
        use std::sync::atomic::Ordering;
        for privacy in [false, true] {
            let (service, store, release, reads) = blocked_capture();
            if privacy {
                service
                    .update_preferences(CapturePreferences {
                        excluded_bundle_ids: vec!["com.synthetic.Editor".into()],
                        ..Default::default()
                    })
                    .expect("request changed policy");
                service
                    .update_preferences(CapturePreferences::default())
                    .expect("request previous policy");
            } else {
                let first = service.pause(None).expect("request pause");
                let latest = service.resume().expect("request resume");
                assert!(latest.revision > first.revision);
                assert_eq!(latest.control_pending, Some(CaptureControlKind::Resume));
            }
            release.send(()).expect("release read");
            wait_until(|| reads.load(Ordering::SeqCst) >= 2);
            let status = service.status().expect("worker confirmation");
            assert!(!status.paused);
            assert!(status.control_pending.is_none());
            assert_eq!(status.pending_items, 0);
            assert!(
                store
                    .list_history(paste_domain::SearchPage::default())
                    .expect("history")
                    .is_empty()
            );
        }
    }

    #[test]
    fn resume_is_pending_until_source_boundary_and_failure_keeps_pause() {
        use paste_platform::{
            CaptureLimits, ClipboardError, ClipboardPoll, ClipboardPrivacyPolicy,
        };
        struct BoundarySource {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<bool>,
        }
        impl ClipboardSource for BoundarySource {
            fn discard_current(&mut self) -> Result<(), ClipboardError> {
                self.entered.send(()).expect("boundary observer");
                if self
                    .release
                    .recv_timeout(StdDuration::from_secs(3))
                    .expect("release boundary")
                {
                    Ok(())
                } else {
                    Err(ClipboardError::Unavailable)
                }
            }
            fn poll(
                &mut self,
                _: &ClipboardPrivacyPolicy,
                _: CaptureLimits,
                _: &DeviceMetadata,
            ) -> Result<ClipboardPoll, ClipboardError> {
                Ok(ClipboardPoll::Unchanged)
            }
        }
        let store = Arc::new(SqliteStore::open_in_memory().expect("history"));
        let device = store.get_or_create_device("Synthetic Mac").expect("device");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let service = CaptureService::spawn_with_source(
            store,
            device,
            CapturePreferences::default(),
            move || BoundarySource {
                entered: entered_tx,
                release: release_rx,
            },
            |_| {},
        )
        .expect("worker");
        service.pause(None).expect("request pause");
        wait_until(|| service.status().expect("pause status").paused);
        for succeeds in [false, true] {
            let response = service.resume().expect("queue resume");
            entered_rx
                .recv_timeout(StdDuration::from_secs(3))
                .expect("boundary blocked");
            let pending = service
                .status()
                .expect("responsive status while boundary blocks");
            // Always release before assertions, including failures.
            release_tx.send(succeeds).expect("release boundary");
            assert!(response.paused);
            assert!(pending.paused);
            assert_eq!(pending.control_pending, Some(CaptureControlKind::Resume));
            wait_until(|| {
                service
                    .status()
                    .expect("confirmed control")
                    .control_pending
                    .is_none()
            });
            let confirmed = service.status().expect("result");
            assert_eq!(confirmed.paused, !succeeds);
            assert!(confirmed.revision > pending.revision);
            if !succeeds {
                assert!(
                    confirmed
                        .last_error
                        .as_deref()
                        .is_some_and(|error| error.contains("该次未解除暂停"))
                );
            }
        }
    }

    #[test]
    fn older_acknowledgements_do_not_clear_a_later_queued_request() {
        struct IdleSource;
        impl ClipboardSource for IdleSource {
            fn discard_current(&mut self) -> Result<(), paste_platform::ClipboardError> {
                Ok(())
            }

            fn poll(
                &mut self,
                _: &paste_platform::ClipboardPrivacyPolicy,
                _: paste_platform::CaptureLimits,
                _: &DeviceMetadata,
            ) -> Result<paste_platform::ClipboardPoll, paste_platform::ClipboardError> {
                Ok(paste_platform::ClipboardPoll::Unchanged)
            }
        }
        let store = Arc::new(SqliteStore::open_in_memory().expect("history"));
        let device = store.get_or_create_device("Synthetic Mac").expect("device");
        let mut coordinator = CaptureCoordinator::new(IdleSource, store, device);
        let (sender, receiver) = mpsc::channel();
        let service = CaptureService {
            sender: Some(sender),
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        };
        for _ in 0..64 {
            service.pause(None).expect("request pause");
        }
        let latest = service
            .resume()
            .expect("last request wins after confirmation");
        let mut stopped = false;
        process_controls(&receiver, &mut coordinator, &service.status, &mut stopped);
        let intermediate = service.status().expect("only older commands processed");
        assert_eq!(
            intermediate.control_pending,
            Some(CaptureControlKind::Resume)
        );
        assert!(intermediate.paused);
        assert!(intermediate.revision > latest.revision);
        process_controls(&receiver, &mut coordinator, &service.status, &mut stopped);
        let confirmed = service.status().expect("latest command processed");
        assert!(confirmed.control_pending.is_none());
        assert!(!confirmed.paused);
        assert!(confirmed.revision > intermediate.revision);
    }

    #[test]
    fn idle_status_does_not_advance_revision_and_failed_send_does_not_claim_pending() {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(CaptureStatus::default()));
        update_status(&status, |_| {});
        assert_eq!(status.lock().expect("status").revision, 0);
        drop(receiver);
        let service = CaptureService {
            sender: Some(sender),
            status,
        };
        assert!(service.pause(None).is_err());
        assert_eq!(
            service.status().expect("unchanged status"),
            CaptureStatus::default()
        );
    }

    #[test]
    fn changing_clipboard_does_not_invent_a_storage_error_or_erase_an_existing_one() {
        let mut status = CaptureStatus::default();
        let changed = Err(CoreError::Clipboard(
            paste_platform::ClipboardError::ChangedDuringRead,
        ));
        record_tick(&mut status, &changed, PauseState::Running, 0);
        assert!(status.last_error.is_none());
        assert!(status.last_change_count.is_none());
        status.last_error = Some("previous real failure".into());
        status.last_change_count = Some(5);
        record_tick(&mut status, &changed, PauseState::Running, 0);
        assert_eq!(status.last_error.as_deref(), Some("previous real failure"));
        assert_eq!(status.last_change_count, Some(5));
        assert_eq!(status.pending_items, 0);
    }

    #[test]
    fn isolated_capture_has_no_control_channel_and_cannot_be_resumed() {
        let service = CaptureService::isolated();
        assert!(service.sender.is_none());
        assert!(service.resume().is_err());
        assert!(service.pause(Some(15)).is_err());
        assert!(
            service
                .update_preferences(CapturePreferences::default())
                .is_err()
        );
        let status = service.status().expect("isolated capture status");
        assert!(status.isolated && status.paused);
        assert!(status.last_change_count.is_none());
    }

    #[test]
    fn capture_errors_survive_idle_and_ignored_ticks_until_a_real_save() {
        let mut status = CaptureStatus::default();
        record_tick(
            &mut status,
            &Err(paste_platform::ClipboardError::Unavailable.into()),
            PauseState::Running,
            0,
        );
        let failure = status.last_error.clone();
        assert!(
            failure
                .as_ref()
                .is_some_and(|message| message.contains("未成功"))
        );
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Unchanged),
            PauseState::Running,
            0,
        );
        assert_eq!(status.last_error, failure);
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Ignored {
                change_count: 8,
                reason: paste_platform::IgnoreReason::Confidential,
            }),
            PauseState::Running,
            0,
        );
        assert_eq!(status.last_error, failure);
        record_tick(
            &mut status,
            &Err(paste_storage::StorageError::CaptureRejected.into()),
            PauseState::Running,
            2,
        );
        let pending_failure = status.last_error.clone();
        assert!(
            pending_failure
                .as_ref()
                .is_some_and(|message| message.contains("2 条") && message.contains("退出会丢失"))
        );
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Unchanged),
            PauseState::Running,
            2,
        );
        assert_eq!(status.last_error, pending_failure);
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Stored {
                change_count: 9,
                clip_ids: vec![],
            }),
            PauseState::Running,
            0,
        );
        assert!(status.last_error.is_none());
        assert_eq!(status.pending_items, 0);
        assert_eq!(status.last_change_count, Some(9));
    }

    #[test]
    fn expired_pause_and_privacy_discard_are_reported_without_claiming_a_save() {
        let mut status = CaptureStatus::default();
        let until = Utc::now() + Duration::minutes(1);
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Unchanged),
            PauseState::Until(until),
            0,
        );
        assert!(status.paused);
        assert_eq!(status.paused_until_ms, Some(until.timestamp_millis()));
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Unchanged),
            PauseState::Running,
            0,
        );
        assert!(!status.paused);
        assert!(status.paused_until_ms.is_none());
        record_tick(
            &mut status,
            &Ok(CaptureEvent::DiscardedForPrivacyChange {
                change_count: 10,
                item_count: 1,
            }),
            PauseState::Running,
            0,
        );
        assert!(
            status
                .last_error
                .as_ref()
                .is_some_and(|message| message.contains("不会补录"))
        );
        record_tick(
            &mut status,
            &Ok(CaptureEvent::Unchanged),
            PauseState::Running,
            0,
        );
        assert!(status.last_error.is_some());
    }

    #[test]
    fn storage_retry_backs_off_without_delaying_controls_or_recovery_reset() {
        let mut retry = CaptureRetry::default();
        let mut now = Instant::now();
        assert!(retry.ready(now, true));
        for delay in [250, 500, 1_000, 2_000, 4_000, 5_000, 5_000] {
            retry.observe(now, true);
            assert!(!retry.ready(now, true));
            assert!(
                retry.ready(now, false),
                "pause/privacy cleared memory; process its event now"
            );
            assert!(!retry.ready(now + StdDuration::from_millis(delay - 1), true));
            now += StdDuration::from_millis(delay);
            assert!(retry.ready(now, true));
        }
        retry.observe(now, false);
        assert!(retry.ready(now, true));
        retry.observe(now, true);
        assert!(!retry.ready(now + StdDuration::from_millis(249), true));
        assert!(retry.ready(now + StdDuration::from_millis(250), true));
    }
}
