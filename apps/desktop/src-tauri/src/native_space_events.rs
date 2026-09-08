//! Scoped notification subscription for the floating clipboard panel.
use objc2::{MainThreadMarker, rc::Retained};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue};

/// A separate notification center owns workspace events. The panel may join
/// every Space, but a user Space change dismisses it until the next invocation.
pub struct ActiveSpaceEvents {
    center: Retained<NSNotificationCenter>,
    token: Retained<objc2::runtime::ProtocolObject<dyn NSObjectProtocol>>,
    _main_thread: MainThreadMarker,
}

impl ActiveSpaceEvents {
    pub fn subscribe(mtm: MainThreadMarker, handler: impl Fn() + Send + Sync + 'static) -> Self {
        use objc2_app_kit::{NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification};
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let block =
            block2::RcBlock::new(move |_notification: std::ptr::NonNull<NSNotification>| {
                handler();
            });
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceActiveSpaceDidChangeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        Self {
            center,
            token,
            _main_thread: mtm,
        }
    }
}

impl Drop for ActiveSpaceEvents {
    fn drop(&mut self) {
        unsafe { self.center.removeObserver((*self.token).as_ref()) };
    }
}
