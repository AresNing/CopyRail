//! Late native event delivery must not revive completed feedback. The native
//! session still validates identity/payload/expiry before it emits any event.
use crate::drag_feedback::{DragLayout, LayoutUpdate};
use std::collections::VecDeque;

/// A single in-flight IPC update, plus the newest unsent geometry. Native
/// session identity and monotonic revision checks remain authoritative.
#[derive(Default)]
pub struct LayoutPublisher {
    session: Option<String>,
    revision: u32,
    last: Option<DragLayout>,
    pending: Option<LayoutUpdate>,
    in_flight: bool,
}

impl LayoutPublisher {
    pub fn observe(&mut self, id: &str, layout: DragLayout) -> Option<LayoutUpdate> {
        if id.is_empty() || id.len() > 64 || !layout.valid() {
            return None;
        }
        if self.session.as_deref() != Some(id) {
            self.session = Some(id.to_owned());
            self.revision = 0;
            self.last = None;
            self.pending = None;
        }
        if self.last.as_ref() == Some(&layout) {
            return None;
        }
        self.revision = self.revision.checked_add(1)?;
        self.last = Some(layout.clone());
        self.pending = Some(LayoutUpdate {
            session_id: id.to_owned(),
            revision: self.revision,
            layout,
        });
        self.take_next()
    }

    fn take_next(&mut self) -> Option<LayoutUpdate> {
        if self.in_flight {
            return None;
        }
        let next = self.pending.take()?;
        self.in_flight = true;
        Some(next)
    }

    pub fn complete(&mut self) -> Option<LayoutUpdate> {
        self.in_flight = false;
        self.take_next()
    }

    pub fn finish(&mut self, id: &str) {
        if self.session.as_deref() == Some(id) {
            self.session = None;
            self.last = None;
            self.pending = None;
        }
    }
}

#[derive(Default)]
pub struct DragLifecycle {
    active: Option<String>,
    closed: VecDeque<(String, bool)>, // true: cancelled or already consumed
}

impl DragLifecycle {
    fn close(&mut self, id: &str, consumed: bool) {
        if let Some(entry) = self.closed.iter_mut().find(|(key, _)| key == id) {
            entry.1 |= consumed;
        } else {
            if self.closed.len() == 32 {
                self.closed.pop_front();
            }
            self.closed.push_back((id.to_owned(), consumed));
        }
    }
    pub fn finish(&mut self, id: &str, cancelled: bool) -> bool {
        if id.is_empty() || id.len() > 64 {
            return false;
        }
        let clear = self.active.as_deref().is_none_or(|current| current == id);
        self.close(id, cancelled);
        if clear {
            self.active = None;
        }
        clear
    }
    /// Some(clear-current-feedback) for an accepted event, None for stale data.
    pub fn route(&mut self, id: &str, phase: &str) -> Option<bool> {
        if id.is_empty() || id.len() > 64 || !matches!(phase, "enter" | "over" | "leave" | "drop") {
            return None;
        }
        let closed = self.closed.iter().find(|(key, _)| key == id);
        if let Some((_, consumed)) = closed {
            if phase != "drop" || *consumed {
                return None;
            }
        } else if self.active.as_deref().is_some_and(|current| current != id) {
            return None;
        }
        let current = self.active.as_deref().is_none_or(|active| active == id);
        if phase == "drop" {
            self.close(id, true);
            if current {
                self.active = None;
            }
        } else if phase != "leave" {
            self.active = Some(id.to_owned());
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_updates_are_coalesced_and_old_completion_only_drains_latest_session() {
        let mut publisher = LayoutPublisher::default();
        let layout = DragLayout {
            width: 1440.0,
            height: 248.0,
            tabs: vec![],
            timeline: None,
        };
        let first = publisher
            .observe("first", layout.clone())
            .expect("initial update");
        assert_eq!(first.revision, 1);
        assert!(publisher.observe("first", layout.clone()).is_none());
        for width in [1400.0, 1300.0, 1200.0] {
            assert!(
                publisher
                    .observe(
                        "first",
                        DragLayout {
                            width,
                            ..layout.clone()
                        }
                    )
                    .is_none()
            );
        }
        let latest = publisher.complete().expect("coalesced latest");
        assert_eq!(latest.revision, 4);
        assert_eq!(latest.layout.width, 1200.0);
        publisher.finish("first");
        assert!(publisher.observe("second", layout.clone()).is_none());
        publisher.finish("first");
        let second = publisher
            .complete()
            .expect("old completion drains successor");
        assert_eq!(second.session_id, "second");
        assert_eq!(second.revision, 1);
        publisher.observe(
            "second",
            DragLayout {
                width: 1300.0,
                ..layout
            },
        );
        publisher.finish("second");
        assert!(
            publisher.complete().is_none(),
            "cancel discards queued layout"
        );
    }
    #[test]
    fn ended_hover_and_old_completion_do_not_corrupt_new_drag() {
        let mut d = DragLifecycle::default();
        assert_eq!(d.route("a", "enter"), Some(true));
        assert!(d.finish("a", false));
        assert_eq!(d.route("a", "over"), None);
        assert_eq!(d.route("b", "enter"), Some(true));
        assert!(!d.finish("a", false));
        assert_eq!(d.route("a", "leave"), None);
        assert_eq!(d.route("a", "drop"), Some(false));
        assert_eq!(d.route("b", "over"), Some(true));
        assert_eq!(d.route("a", "drop"), None);
    }
    #[test]
    fn cancellation_blocks_all_late_events_and_history_is_bounded() {
        let mut d = DragLifecycle::default();
        d.route("a", "enter");
        d.finish("a", true);
        for phase in ["enter", "over", "leave", "drop"] {
            assert_eq!(d.route("a", phase), None);
        }
        for n in 0..100 {
            d.finish(&n.to_string(), true);
        }
        assert_eq!(d.closed.len(), 32);
        assert_eq!(d.route("", "enter"), None);
        assert_eq!(d.route("b", "invalid"), None);
    }
}
