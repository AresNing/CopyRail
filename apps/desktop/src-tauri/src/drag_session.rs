//! Routes only our active native export back into Pinboards. Other file drops
//! are never interpreted as clip IDs. No clipboard contents cross this event.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use paste_domain::ClipId;
use serde::Serialize;
use tauri::WebviewWindow;
#[cfg(not(target_os = "macos"))]
use tauri::{DragDropEvent, Emitter, WindowEvent};
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragEndedEvent {
    pub session_id: Uuid,
    pub cancelled: bool,
}

pub struct Feedback {
    pub layout: crate::drag_feedback::DragLayout,
    pub labels: std::collections::HashMap<paste_domain::PinboardId, String>,
    pub count: usize,
    pub revision: u32,
    pub dragged: Vec<ClipId>,
}

impl Feedback {
    pub fn from_context(
        mut layout: crate::drag_feedback::DragLayout,
        context: paste_storage::ClipActionContext,
    ) -> Self {
        let movable = context
            .clips
            .iter()
            .all(|c| c.writable && !c.move_restricted);
        let labels: std::collections::HashMap<_, _> = context
            .boards
            .into_iter()
            .filter(|b| movable && b.writable)
            .map(|b| {
                (
                    b.id,
                    b.name
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .take(32)
                        .collect(),
                )
            })
            .collect();
        layout.tabs.retain(|t| labels.contains_key(&t.id));
        if layout
            .timeline
            .as_ref()
            .is_some_and(|t| !labels.contains_key(&t.pinboard_id))
        {
            layout.timeline = None;
        }
        Self {
            layout,
            labels,
            count: context.clips.len(),
            revision: 0,
            dragged: context.clips.iter().map(|c| c.clip.id).collect(),
        }
    }

    pub fn refresh(&mut self, revision: u32, mut layout: crate::drag_feedback::DragLayout) -> bool {
        if revision <= self.revision || !layout.valid() {
            return false;
        }
        // New visibility does not create new movement permission or names.
        layout.tabs.retain(|t| self.labels.contains_key(&t.id));
        if layout
            .timeline
            .as_ref()
            .is_some_and(|t| !self.labels.contains_key(&t.pinboard_id))
        {
            layout.timeline = None;
        }
        self.layout = layout;
        self.revision = revision;
        true
    }

    pub fn target(
        &self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Option<crate::drag_feedback::TargetHit> {
        self.layout.target(x, y, width, height, &self.dragged)
    }
}

#[cfg(target_os = "macos")]
#[path = "drag_destination.rs"]
mod native;
#[cfg(target_os = "macos")]
pub use native::{
    begin as begin_native_destination, end as end_native_destination,
    update as update_native_feedback,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalDragEvent {
    phase: &'static str,
    session_id: Uuid,
    clip_ids: Vec<ClipId>,
    x: f64,
    y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tab_feedback: Option<crate::drag_feedback::TabFeedback>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placement_feedback: Option<crate::drag_feedback::PlacementFeedback>,
}

struct Session {
    id: Uuid,
    paths: Vec<PathBuf>,
    clips: Vec<ClipId>,
    expires: Instant,
    entered: bool,
    feedback_open: bool,
}

#[derive(Default)]
pub struct DragSessions(Mutex<Option<Session>>);

impl DragSessions {
    pub fn feedback_is_open(&self, id: Uuid, now: Instant) -> bool {
        self.0.lock().ok().is_some_and(|state| {
            state.as_ref().is_some_and(|session| {
                session.id == id && session.feedback_open && now < session.expires
            })
        })
    }
    pub fn cancel(&self, id: Uuid) {
        if let Ok(mut state) = self.0.lock()
            && state.as_ref().is_some_and(|session| session.id == id)
        {
            *state = None;
        }
    }
    pub fn start(
        &self,
        paths: Vec<PathBuf>,
        clips: Vec<ClipId>,
        now: Instant,
    ) -> Result<Uuid, &'static str> {
        if paths.is_empty() || clips.is_empty() {
            return Err("empty drag session");
        }
        let id = Uuid::new_v4();
        *self.0.lock().map_err(|_| "drag session lock unavailable")? = Some(Session {
            id,
            paths,
            clips,
            expires: now + Duration::from_secs(120),
            entered: false,
            feedback_open: true,
        });
        Ok(id)
    }

    pub fn finish(&self, id: Uuid, now: Instant) {
        // AppKit can end the native session before Tauri dispatches the queued
        // destination drop. Keep a short grace period; a new session is untouched.
        if let Ok(mut state) = self.0.lock()
            && let Some(session) = state.as_mut().filter(|session| session.id == id)
        {
            session.expires = now + Duration::from_secs(1);
            session.feedback_open = false;
        }
    }

    #[cfg(any(test, not(target_os = "macos")))]
    fn route(
        &self,
        phase: &'static str,
        paths: Option<&[PathBuf]>,
        x: f64,
        y: f64,
        now: Instant,
    ) -> Option<InternalDragEvent> {
        self.route_for(None, phase, paths, (x, y), now)
    }

    fn route_for(
        &self,
        expected: Option<Uuid>,
        phase: &'static str,
        paths: Option<&[PathBuf]>,
        (x, y): (f64, f64),
        now: Instant,
    ) -> Option<InternalDragEvent> {
        let mut state = self.0.lock().ok()?;
        let session = state.as_mut()?;
        if expected.is_some_and(|id| id != session.id) {
            return None;
        }
        if now >= session.expires {
            *state = None;
            return None;
        }
        if let Some(paths) = paths
            && paths != session.paths
        {
            session.entered = false;
            return None;
        }
        if phase == "enter" {
            session.entered = true;
        }
        if !session.entered && phase == "over" {
            return None;
        }
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let event = InternalDragEvent {
            phase,
            session_id: session.id,
            clip_ids: session.clips.clone(),
            x,
            y,
            tab_feedback: None,
            placement_feedback: None,
        };
        if phase == "leave" {
            session.entered = false;
        }
        if phase == "drop" {
            *state = None;
        }
        Some(event)
    }
}

#[cfg(target_os = "macos")]
pub fn install(_window: &WebviewWindow, _sessions: Arc<DragSessions>, _diagnostics: bool) {
    // Native destinations are attached only during our own active source drag.
}

#[cfg(not(target_os = "macos"))]
pub fn install(window: &WebviewWindow, sessions: Arc<DragSessions>, diagnostics: bool) {
    let destination = window.clone();
    // A WebviewWindow's file drops are synthesized as WindowEvent, not
    // WebviewEvent. Listen to this surface once to avoid duplicate commits.
    window.on_window_event(move |event| {
        let WindowEvent::DragDrop(event) = event else {
            return;
        };
        let (phase, paths, position) = match event {
            DragDropEvent::Enter { paths, position } => {
                ("enter", Some(paths.as_slice()), Some(position))
            }
            DragDropEvent::Over { position } => ("over", None, Some(position)),
            DragDropEvent::Drop { paths, position } => {
                ("drop", Some(paths.as_slice()), Some(position))
            }
            DragDropEvent::Leave => ("leave", None, None),
            _ => return,
        };
        // Wry 0.55's WKWebView bridge already emits view points (despite the
        // PhysicalPosition type). Dividing on Retina would halve the hit target.
        #[cfg(target_os = "macos")]
        let scale = 1.0;
        #[cfg(not(target_os = "macos"))]
        let scale = destination.scale_factor().unwrap_or(1.0);
        let (x, y) = position.map_or((0.0, 0.0), |p| (p.x / scale, p.y / scale));
        if diagnostics {
            eprintln!(
                "Native UI test drag: phase={phase} x={x} y={y} file_count={}",
                paths.map_or(0, <[PathBuf]>::len)
            );
        }
        if let Some(payload) = sessions.route(phase, paths, x, y, Instant::now()) {
            if diagnostics {
                eprintln!(
                    "Native UI test routed: {phase} clips={}",
                    payload.clip_ids.len()
                );
            }
            let _ = destination.emit("pasters-internal-drag", payload);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_updates_reject_finished_cancelled_expired_and_replaced_sessions() {
        let sessions = DragSessions::default();
        let now = Instant::now();
        let start = || {
            sessions
                .start(
                    vec![PathBuf::from("/synthetic/export.txt")],
                    vec![ClipId::new()],
                    now,
                )
                .expect("session")
        };
        let first = start();
        assert!(sessions.feedback_is_open(first, now));
        assert!(!sessions.feedback_is_open(first, now + Duration::from_secs(120)));
        sessions.finish(first, now);
        assert!(
            !sessions.feedback_is_open(first, now),
            "drop grace must not keep feedback open"
        );
        let second = start();
        assert!(!sessions.feedback_is_open(first, now));
        sessions.cancel(first);
        assert!(sessions.feedback_is_open(second, now));
        sessions.cancel(second);
        assert!(!sessions.feedback_is_open(second, now));
        let third = start();
        sessions.route("drop", None, 1.0, 2.0, now);
        assert!(!sessions.feedback_is_open(third, now));
    }

    #[test]
    fn refreshed_feedback_is_monotonic_validated_and_cannot_grant_permission() {
        let id = paste_domain::PinboardId::new();
        let unknown = paste_domain::PinboardId::new();
        let layout = crate::drag_feedback::DragLayout {
            width: 1440.0,
            height: 248.0,
            tabs: vec![],
            timeline: None,
        };
        let mut f = Feedback {
            layout: layout.clone(),
            labels: [(id, "Work".into())].into(),
            count: 1,
            revision: 0,
            dragged: vec![],
        };
        let tab = crate::drag_feedback::DragTab {
            id,
            x: 600.0,
            y: 11.0,
            width: 52.0,
            height: 27.0,
        };
        let refreshed = crate::drag_feedback::DragLayout {
            tabs: vec![
                tab.clone(),
                crate::drag_feedback::DragTab {
                    id: unknown,
                    x: 700.0,
                    ..tab
                },
            ],
            ..layout.clone()
        };
        assert!(f.refresh(2, refreshed));
        assert_eq!(f.layout.tabs.len(), 1);
        assert_eq!(f.layout.tabs[0].id, id);
        assert_eq!(f.labels.len(), 1);
        assert!(!f.refresh(1, layout.clone()));
        assert!(!f.refresh(2, layout.clone()));
        assert!(!f.refresh(
            3,
            crate::drag_feedback::DragLayout {
                width: f64::NAN,
                ..layout.clone()
            }
        ));
        assert_eq!(f.revision, 2);
        assert_eq!(f.layout.tabs.len(), 1);
        assert!(f.refresh(4, layout));
        assert!(f.layout.tabs.is_empty(), "offscreen target is cleared");
        let timeline = crate::drag_feedback::DragTimeline {
            pinboard_id: unknown,
            bounds: crate::drag_feedback::DragRect {
                x: 20.0,
                y: 48.0,
                width: 1400.0,
                height: 185.0,
            },
            cards: vec![],
        };
        let update = crate::drag_feedback::DragLayout {
            width: 1440.0,
            height: 248.0,
            tabs: vec![],
            timeline: Some(timeline.clone()),
        };
        assert!(f.refresh(5, update.clone()));
        assert!(
            f.layout.timeline.is_none(),
            "unwritable/unknown board cannot acquire reorder targets"
        );
        assert!(f.target(300.0, 125.0, 1440.0, 248.0).is_none());
        assert!(f.refresh(
            6,
            crate::drag_feedback::DragLayout {
                timeline: Some(crate::drag_feedback::DragTimeline {
                    pinboard_id: id,
                    ..timeline
                }),
                ..update
            }
        ));
        assert!(
            f.target(300.0, 125.0, 1440.0, 248.0).is_some(),
            "writable empty board can append"
        );
    }

    #[test]
    fn cancelled_session_has_no_drop_grace_and_cannot_cancel_its_successor() {
        let sessions = DragSessions::default();
        let now = Instant::now();
        let paths = vec![PathBuf::from("/synthetic/export.txt")];
        let old = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("session");
        sessions.cancel(old);
        assert!(
            sessions
                .route("drop", Some(&paths), 640.0, 25.0, now)
                .is_none()
        );
        let new = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("new session");
        sessions.cancel(old);
        assert_eq!(
            sessions
                .route("enter", Some(&paths), 640.0, 25.0, now)
                .expect("new is live")
                .session_id,
            new
        );
    }

    #[test]
    fn feedback_uses_existing_writable_board_names_not_the_layout_as_authority() {
        let store = paste_storage::SqliteStore::open_in_memory().expect("store");
        let device = store.get_or_create_device("Synthetic").expect("device");
        let board = store.create_pinboard("Work", "#ff9500").expect("board");
        let clip = store
            .create_textual_item_in_pinboard(
                paste_domain::ContentKind::Text,
                "Synthetic",
                paste_domain::SourceApplication::unknown(),
                device,
                Some(board.id),
            )
            .expect("clip");
        let context = store.clip_action_context(&[clip.id]).expect("snapshot");
        let layout = crate::drag_feedback::DragLayout {
            width: 1440.0,
            height: 248.0,
            tabs: vec![
                crate::drag_feedback::DragTab {
                    id: board.id,
                    x: 612.0,
                    y: 11.0,
                    width: 52.0,
                    height: 27.0,
                },
                crate::drag_feedback::DragTab {
                    id: paste_domain::PinboardId::new(),
                    x: 670.0,
                    y: 11.0,
                    width: 52.0,
                    height: 27.0,
                },
            ],
            timeline: None,
        };
        let feedback = Feedback::from_context(layout.clone(), context);
        assert_eq!(feedback.layout.tabs.len(), 1);
        assert_eq!(feedback.labels[&board.id], "Work");
        assert_eq!(feedback.count, 1);
        let mut context = store.clip_action_context(&[clip.id]).expect("snapshot");
        context.boards[0].writable = false;
        assert!(
            Feedback::from_context(layout.clone(), context)
                .layout
                .tabs
                .is_empty()
        );
        let mut context = store.clip_action_context(&[clip.id]).expect("snapshot");
        context.clips[0].writable = false;
        assert!(
            Feedback::from_context(layout, context)
                .layout
                .tabs
                .is_empty()
        );
    }

    #[test]
    fn a_stale_native_destination_cannot_consume_a_new_session_with_same_paths() {
        let sessions = DragSessions::default();
        let now = Instant::now();
        let paths = vec![PathBuf::from("/synthetic/original-file.txt")];
        let old = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("first synthetic drag");
        sessions.finish(old, now);
        let new = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("replacement synthetic drag");
        assert!(
            sessions
                .route_for(Some(old), "drop", Some(&paths), (1.0, 2.0), now)
                .is_none()
        );
        assert!(
            sessions
                .route_for(Some(new), "enter", Some(&paths), (1.0, 2.0), now)
                .is_some()
        );
        assert!(
            sessions
                .route_for(Some(new), "drop", Some(&paths), (1.0, 2.0), now)
                .is_some()
        );
        assert!(
            sessions
                .route_for(Some(new), "drop", Some(&paths), (1.0, 2.0), now)
                .is_none()
        );
    }

    #[test]
    fn foreign_paths_are_rejected_and_a_drop_is_consumed_once() {
        let sessions = DragSessions::default();
        let now = Instant::now();
        let paths = vec![PathBuf::from("/synthetic/export.txt")];
        let ids = vec![ClipId::new(), ClipId::new()];
        let id = sessions
            .start(paths.clone(), ids.clone(), now)
            .expect("synthetic fixture operation");
        assert!(sessions.route("over", None, 1.0, 2.0, now).is_none());
        assert!(
            sessions
                .route("enter", Some(&[PathBuf::from("/foreign")]), 1.0, 2.0, now)
                .is_none()
        );
        let event = sessions
            .route("enter", Some(&paths), 320.0, 60.0, now)
            .expect("synthetic fixture operation");
        assert_eq!(event.clip_ids, ids);
        assert_eq!(event.session_id, id);
        assert_eq!((event.x, event.y), (320.0, 60.0));
        assert!(sessions.route("over", None, f64::NAN, 2.0, now).is_none());
        assert!(
            sessions
                .route("drop", Some(&paths), 320.0, 60.0, now)
                .is_some()
        );
        assert!(
            sessions
                .route("drop", Some(&paths), 320.0, 60.0, now)
                .is_none()
        );
    }

    #[test]
    fn queued_drop_has_a_bounded_grace_and_old_completion_cannot_replace_new_session() {
        let sessions = DragSessions::default();
        let now = Instant::now();
        let paths = vec![PathBuf::from("/synthetic")];
        let first = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("synthetic fixture operation");
        sessions.finish(first, now);
        assert!(
            sessions
                .route(
                    "drop",
                    Some(&paths),
                    0.0,
                    0.0,
                    now + Duration::from_millis(100)
                )
                .is_some()
        );
        let second = sessions
            .start(paths.clone(), vec![ClipId::new()], now)
            .expect("synthetic fixture operation");
        sessions.finish(first, now);
        assert_eq!(
            sessions
                .route(
                    "enter",
                    Some(&paths),
                    0.0,
                    0.0,
                    now + Duration::from_secs(2)
                )
                .expect("synthetic fixture operation")
                .session_id,
            second
        );
        sessions.finish(second, now + Duration::from_secs(2));
        assert!(
            sessions
                .route("drop", Some(&paths), 0.0, 0.0, now + Duration::from_secs(3))
                .is_none()
        );
    }
}
