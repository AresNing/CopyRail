//! App-local display notifications with explicit observer lifetime. No global
//! event monitor, polling, application activation or window ordering.
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSWindowDidChangeScreenNotification,
    NSWindowDidEndLiveResizeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutEvent {
    DisplaysChanged,
    WindowScreenChanged,
    Resized,
    ResizeEnded,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LayoutReaction {
    pub remember_user_size: bool,
    pub reposition: bool,
    pub defer_display: bool,
}

pub fn layout_reaction(
    event: LayoutEvent,
    visible: bool,
    application_hidden: bool,
    live_resize: bool,
    preview: bool,
    display_pending: bool,
) -> LayoutReaction {
    LayoutReaction {
        remember_user_size: event == LayoutEvent::ResizeEnded && !preview,
        // Leave hidden windows hidden, and do not fight a resize gesture.
        // User-resized readers retain their size until a display change or
        // the next preview; only the dock snaps back after a resize ends.
        reposition: visible
            && !application_hidden
            && !live_resize
            && (!matches!(event, LayoutEvent::Resized | LayoutEvent::ResizeEnded)
                || !preview
                || display_pending),
        defer_display: live_resize
            && matches!(
                event,
                LayoutEvent::DisplaysChanged | LayoutEvent::WindowScreenChanged
            ),
    }
}

pub struct WindowEvents {
    center: Retained<NSNotificationCenter>,
    tokens: Vec<Retained<objc2::runtime::ProtocolObject<dyn NSObjectProtocol>>>,
    // Keep identity filters alive until Drop unregisters every observer.
    _senders: [Retained<AnyObject>; 2],
    _main_thread: MainThreadMarker,
}

impl WindowEvents {
    /// `application` and `window` are identity filters, not payloads we read.
    /// They are retained until these observations are removed.
    pub fn subscribe(
        mtm: MainThreadMarker,
        center: Retained<NSNotificationCenter>,
        application: &AnyObject,
        window: &AnyObject,
        handler: impl Fn(LayoutEvent) + Send + Sync + 'static,
    ) -> Self {
        let handler = Arc::new(handler);
        let queue = NSOperationQueue::mainQueue();
        let senders = [application, window].map(|object| {
            // Both arguments are borrowed live Objective-C objects. Retain
            // before registering, and release only after removing tokens.
            unsafe { Retained::retain(object as *const AnyObject as *mut AnyObject) }
                .expect("borrowed notification sender is non-null")
        });
        let mut tokens = Vec::new();
        for (name, object, event) in unsafe {
            [
                (
                    NSApplicationDidChangeScreenParametersNotification,
                    application,
                    LayoutEvent::DisplaysChanged,
                ),
                (
                    NSWindowDidChangeScreenNotification,
                    window,
                    LayoutEvent::WindowScreenChanged,
                ),
                (
                    NSWindowDidEndLiveResizeNotification,
                    window,
                    LayoutEvent::ResizeEnded,
                ),
            ]
        } {
            let handler = handler.clone();
            let block =
                block2::RcBlock::new(move |_notification: std::ptr::NonNull<NSNotification>| {
                    handler(event);
                });
            // The block is sendable, and Foundation delivers it on the main
            // queue. The exact object filters prevent other windows' resize
            // notifications from modifying this panel's preferred size.
            tokens.push(unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(object),
                    Some(&queue),
                    &block,
                )
            });
        }
        Self {
            center,
            tokens,
            _senders: senders,
            _main_thread: mtm,
        }
    }
}

impl Drop for WindowEvents {
    fn drop(&mut self) {
        for token in self.tokens.drain(..) {
            // Only tokens created on this exact center are removed.
            unsafe { self.center.removeObserver((*token).as_ref()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_or_live_windows_never_reposition_from_notifications() {
        for event in [
            LayoutEvent::DisplaysChanged,
            LayoutEvent::WindowScreenChanged,
            LayoutEvent::Resized,
            LayoutEvent::ResizeEnded,
        ] {
            for preview in [false, true] {
                for (visible, hidden, live) in [
                    (false, false, false),
                    (true, true, false),
                    (true, false, true),
                ] {
                    assert!(
                        !layout_reaction(event, visible, hidden, live, preview, false).reposition
                    );
                }
            }
        }
    }

    #[test]
    fn display_events_reflow_both_modes_but_never_capture_user_size() {
        for event in [
            LayoutEvent::DisplaysChanged,
            LayoutEvent::WindowScreenChanged,
        ] {
            for preview in [false, true] {
                assert_eq!(
                    layout_reaction(event, true, false, false, preview, false),
                    LayoutReaction {
                        remember_user_size: false,
                        reposition: true,
                        defer_display: false
                    }
                );
            }
        }
        assert_eq!(
            layout_reaction(LayoutEvent::ResizeEnded, true, false, false, false, false),
            LayoutReaction {
                remember_user_size: true,
                reposition: true,
                defer_display: false
            }
        );
        assert_eq!(
            layout_reaction(LayoutEvent::ResizeEnded, true, false, false, true, false),
            LayoutReaction {
                remember_user_size: false,
                reposition: false,
                defer_display: false
            }
        );
    }

    #[test]
    fn display_changes_during_reader_resize_are_deferred_until_the_gesture_ends() {
        let during = layout_reaction(LayoutEvent::DisplaysChanged, true, false, true, true, false);
        assert!(during.defer_display && !during.reposition && !during.remember_user_size);
        assert!(
            layout_reaction(LayoutEvent::ResizeEnded, true, false, false, true, true).reposition
        );
        assert!(
            !layout_reaction(LayoutEvent::ResizeEnded, true, true, false, true, true).reposition
        );
    }
}
