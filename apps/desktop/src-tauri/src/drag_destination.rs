//! A transparent destination covering this app only while its native source
//! drag is active. It does not replace the exported files or read the general
//! clipboard. OS destination callbacks, never source completion, trigger drops.
use super::DragSessions;
use crate::drag_feedback::{TargetHit, TargetVisual};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSDragOperation, NSDraggingDestination, NSDraggingFormation,
    NSDraggingInfo, NSDraggingItem, NSDraggingItemEnumerationOptions, NSImage, NSView,
    NSWindowOrderingMode, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSURL,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use tauri::{Emitter, WebviewWindow};
use uuid::Uuid;

struct DestinationState {
    window: WebviewWindow,
    sessions: Arc<DragSessions>,
    id: Uuid,
    diagnostics: bool,
    feedback: RefCell<Option<super::Feedback>>,
    image_dirty: Cell<bool>,
    last_point: Cell<Option<NSPoint>>,
    highlighted: Cell<Option<TargetHit>>,
    preview: Retained<NSImage>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DestinationState]
    struct PasteDropDestination;
    unsafe impl NSObjectProtocol for PasteDropDestination {}
    impl PasteDropDestination {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: objc2_foundation::NSRect) {
            let state = self.ivars();
            if let Some(feedback) = state.feedback.borrow().as_ref()
                && let Some(hit) = state.highlighted.get() {
                let bounds = self.bounds();
                match hit.visual {
                    TargetVisual::Tab => {
                        if let Some(tab) = feedback.layout.tabs.iter().find(|t| t.id == hit.placement.pinboard_id)
                            && let Some(label) = feedback.labels.get(&hit.placement.pinboard_id) {
                            crate::native_drop_feedback::paint(tab, label, feedback.count, bounds.size.width, bounds.size.height);
                        }
                    }
                    TargetVisual::Insertion { x, y, height } => crate::native_drop_feedback::paint_insertion(x, y, height, feedback.count, bounds.size.width, bounds.size.height),
                }
            }
        }
    }
    unsafe impl NSDraggingDestination for PasteDropDestination {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if self.route("enter", sender) { NSDragOperation::Copy } else { NSDragOperation::None }
        }
        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if self.route("over", sender) { NSDragOperation::Copy } else { NSDragOperation::None }
        }
        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if let Some(sender) = sender { self.route("leave", sender); } else { self.clear_feedback(); }
        }
        #[unsafe(method(prepareForDragOperation:))]
        fn prepare(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            // Recheck the actual payload just before AppKit commits a drop.
            let accepted = self.route("over", sender);
            sender.setAnimatesToDestination(accepted && self.ivars().highlighted.get().is_some()
                && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion());
            accepted
        }
        #[unsafe(method(performDragOperation:))]
        fn perform(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.route("drop", sender)
        }
        #[unsafe(method(wantsPeriodicDraggingUpdates))]
        fn periodic(&self) -> bool { true }
    }
);

thread_local! {
    static ACTIVE: RefCell<Option<Retained<PasteDropDestination>>> = const { RefCell::new(None) };
}

fn local_path(value: &str) -> Option<PathBuf> {
    if value.len() > 16_384 {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "file"
        || url.host_str().is_some_and(|host| host != "localhost")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.to_file_path().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().contains(&0) {
            return None;
        }
    }
    path.is_absolute().then_some(path)
}

fn paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<Vec<PathBuf>> {
    // Only the pasteboard supplied to this drag callback; never generalPasteboard.
    let pasteboard = sender.draggingPasteboard();
    let items = pasteboard.pasteboardItems()?;
    if items.is_empty() || items.len() > crate::drag_export::MAX_DRAG_FILES {
        return None;
    }
    let kind = NSString::from_str("public.file-url");
    items
        .iter()
        .map(|item| {
            let value = item.stringForType(&kind)?;
            if value.len() > 16_384 {
                return None;
            }
            local_path(&value.to_string())
        })
        .collect()
}

impl PasteDropDestination {
    fn clear_feedback(&self) {
        let state = self.ivars();
        state.last_point.set(None);
        state.image_dirty.set(true);
        if state.highlighted.take().is_some() {
            self.setNeedsDisplay(true);
            self.displayIfNeeded();
        }
    }

    fn image_feedback(
        &self,
        sender: &ProtocolObject<dyn NSDraggingInfo>,
        point: NSPoint,
        target: Option<TargetHit>,
        dropping: bool,
    ) {
        let state = self.ivars();
        let feedback = state.feedback.borrow();
        let tab = feedback.as_ref().and_then(|f| {
            f.layout.tabs.iter().find(|t| {
                target.is_some_and(|hit| {
                    hit.visual == TargetVisual::Tab && t.id == hit.placement.pinboard_id
                })
            })
        });
        let size = state.preview.size();
        let view = self.bounds().size;
        let frame = if let Some(tab) = tab {
            let width = if dropping {
                20.0
            } else {
                72.0_f64.min(
                    ((view.height - tab.y - tab.height - 62.0).max(1.0)) * size.width
                        / size.height.max(1.0),
                )
            };
            let height = width * size.height / size.width.max(1.0);
            NSRect::new(
                NSPoint::new(
                    (tab.x + tab.width / 2.0 - width / 2.0)
                        .clamp(2.0, (view.width - width - 2.0).max(2.0)),
                    if dropping {
                        tab.y + tab.height / 2.0 - height / 2.0
                    } else {
                        tab.y + tab.height + 54.0
                    },
                ),
                NSSize::new(width, height),
            )
        } else if let Some(TargetHit {
            visual:
                TargetVisual::Insertion {
                    x,
                    y,
                    height: line_height,
                },
            ..
        }) = target
        {
            let width = if dropping { 20.0 } else { 72.0 };
            let height = width * size.height / size.width.max(1.0);
            let x = if dropping {
                x - width / 2.0
            } else if x + width + 12.0 < view.width {
                x + 12.0
            } else {
                x - width - 12.0
            };
            NSRect::new(
                NSPoint::new(
                    x.clamp(2.0, (view.width - width - 2.0).max(2.0)),
                    (y + line_height / 2.0 - height / 2.0)
                        .clamp(2.0, (view.height - height - 2.0).max(2.0)),
                ),
                NSSize::new(width, height),
            )
        } else {
            NSRect::new(NSPoint::new(point.x + 16.0, point.y + 16.0), size)
        };
        let image = state.preview.clone();
        let block = block2::RcBlock::new(
            move |item: std::ptr::NonNull<NSDraggingItem>,
                  _index: isize,
                  _stop: std::ptr::NonNull<objc2::runtime::Bool>| {
                // Item is valid only inside this synchronous enumeration; do not
                // retain it. Contents are our own rendered NSImage, never payload data.
                unsafe {
                    item.as_ref()
                        .setDraggingFrame_contents(frame, Some(image.as_ref()));
                }
            },
        );
        sender.setDraggingFormation(NSDraggingFormation::None);
        // Enumerate only URL writers from our identity-validated file export.
        unsafe {
            sender.enumerateDraggingItemsWithOptions_forView_classes_searchOptions_usingBlock(
                NSDraggingItemEnumerationOptions::empty(),
                Some(self),
                &NSArray::from_slice(&[NSURL::class()]),
                &NSDictionary::new(),
                &block,
            );
        }
    }

    fn route(&self, phase: &'static str, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        let state = self.ivars();
        let Some(paths) = paths(sender) else {
            self.clear_feedback();
            if state.diagnostics {
                eprintln!(
                    "Native UI test native destination: rejected file URL payload phase={phase}"
                );
            }
            return false;
        };
        let point = self.convertPoint_fromView(sender.draggingLocation(), None);
        let mut payload = state.sessions.route_for(
            Some(state.id),
            phase,
            Some(&paths),
            (point.x, point.y),
            Instant::now(),
        );
        let copy_allowed = sender
            .draggingSourceOperationMask()
            .contains(NSDragOperation::Copy);
        let target =
            if payload.is_some() && copy_allowed && matches!(phase, "enter" | "over" | "drop") {
                state.feedback.borrow().as_ref().and_then(|feedback| {
                    let size = self.bounds().size;
                    feedback.target(point.x, point.y, size.width, size.height)
                })
            } else {
                None
            };
        state.last_point.set(
            (payload.is_some() && copy_allowed && matches!(phase, "enter" | "over"))
                .then_some(point),
        );
        if let Some(payload) = payload.as_mut()
            && state.feedback.borrow().is_some()
        {
            payload.tab_feedback = Some(crate::drag_feedback::TabFeedback {
                target: target
                    .filter(|t| t.visual == TargetVisual::Tab)
                    .map(|t| t.placement.pinboard_id),
            });
            payload.placement_feedback = Some(crate::drag_feedback::PlacementFeedback {
                target: target.map(|t| t.placement),
            });
        }
        let previous = state.highlighted.get();
        if payload.is_some()
            && (state.image_dirty.replace(false)
                || previous != target
                || target.is_some()
                || phase == "enter"
                || phase == "drop")
        {
            self.image_feedback(sender, point, target, phase == "drop");
        }
        let visible = if phase == "drop" { None } else { target };
        if state.highlighted.replace(visible) != visible {
            self.setNeedsDisplay(true);
            // Draw during AppKit's tracking callback, not after WebView event
            // dispatch. No content or source titles are written to diagnostics.
            self.displayIfNeeded();
            if state.diagnostics {
                eprintln!(
                    "Native UI test target feedback: visible={}",
                    visible.is_some()
                );
            }
        }
        if state.diagnostics {
            eprintln!(
                "Native UI test native destination: phase={phase} x={} y={} files={} routed={} accepted={}",
                point.x,
                point.y,
                paths.len(),
                payload.is_some(),
                target.is_some()
            );
        }
        // Keep delivering hover over invalid regions so scrolling/layout can
        // refresh. A routed identity is not permission to accept the drop.
        let delivered = payload
            .is_some_and(|payload| state.window.emit("pasters-internal-drag", payload).is_ok());
        delivered && target.is_some()
    }
}

pub fn begin(
    window: &WebviewWindow,
    sessions: Arc<DragSessions>,
    id: Uuid,
    diagnostics: bool,
    feedback: Option<super::Feedback>,
    preview_bytes: &[u8],
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("原生落点必须在主线程创建。")?;
    if ACTIVE.with(|active| active.borrow().is_some()) {
        return Err("已有原生拖动尚未结束。".into());
    }
    let pointer = window.ns_view().map_err(|error| error.to_string())?;
    let parent = unsafe { (pointer as *const NSView).as_ref() }.ok_or("窗口已关闭。")?;
    let preview = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(preview_bytes))
        .ok_or("无法读取拖动预览。")?;
    let view: Retained<PasteDropDestination> = unsafe {
        msg_send![super(PasteDropDestination::alloc(mtm).set_ivars(DestinationState {
            window: window.clone(), sessions, id, diagnostics, feedback: RefCell::new(feedback), image_dirty: Cell::new(false), last_point: Cell::new(None), highlighted: Cell::new(None), preview,
        })), initWithFrame: parent.bounds()]
    };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    view.registerForDraggedTypes(&NSArray::from_retained_slice(&[NSString::from_str(
        "public.file-url",
    )]));
    parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
    if diagnostics {
        eprintln!(
            "Native UI test native destination attached: size={:?}",
            view.bounds().size
        );
    }
    ACTIVE.with(|active| *active.borrow_mut() = Some(view));
    Ok(())
}

pub fn end(id: Uuid) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        if active.as_ref().is_some_and(|view| view.ivars().id == id)
            && let Some(view) = active.take()
        {
            view.removeFromSuperview();
            if view.ivars().diagnostics {
                eprintln!("Native UI test native destination removed");
            }
            return true;
        }
        false
    })
}

/// Called only on the AppKit thread. No drag item or dragging-info object is
/// retained across callbacks; the next periodic callback moves the preview.
pub fn update(id: Uuid, revision: u32, layout: crate::drag_feedback::DragLayout) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(view) = active.as_ref().filter(|view| view.ivars().id == id) else {
            return false;
        };
        let state = view.ivars();
        if !state.sessions.feedback_is_open(id, Instant::now()) {
            return false;
        }
        let changed = state
            .feedback
            .borrow_mut()
            .as_mut()
            .is_some_and(|feedback| feedback.refresh(revision, layout));
        if changed {
            // Repaint against AppKit's last sample now, avoiding a blank frame
            // on every autoscroll tick. Only the next real callback can update
            // NSDraggingItem; never keep it (or the sender) outside enumeration.
            let target = state.last_point.get().and_then(|point| {
                let size = view.bounds().size;
                state
                    .feedback
                    .borrow()
                    .as_ref()
                    .and_then(|f| f.target(point.x, point.y, size.width, size.height))
            });
            state.highlighted.set(target);
            state.image_dirty.set(true);
            view.setNeedsDisplay(true);
            view.displayIfNeeded();
            if state.diagnostics {
                eprintln!("Native UI test target layout refreshed: revision={revision}");
            }
        }
        changed
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_file_urls_are_decoded_without_network_or_path_guessing() {
        assert_eq!(
            local_path("file:///synthetic/a%20b%23%E4%B8%AD.txt"),
            Some(PathBuf::from("/synthetic/a b#中.txt"))
        );
        assert_eq!(
            local_path("file://localhost/synthetic/a"),
            Some(PathBuf::from("/synthetic/a"))
        );
        for value in [
            "https://example.invalid/a",
            "file://remote/share/a",
            "file:///a?x=1",
            "file:///a#part",
            "file:///a%00b",
            "/local/path",
        ] {
            assert!(local_path(value).is_none(), "{value}");
        }
        assert!(local_path(&format!("file:///{}", "a".repeat(16_384))).is_none());
    }
}
