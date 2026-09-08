//! Inspect AppKit frame quantization without showing a window or activating an
//! application. Uses only newly allocated, empty NSWindows; no user UI access.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let mtm = MainThreadMarker::new().ok_or("main thread required")?;
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    if app.activationPolicy() != NSApplicationActivationPolicy::Prohibited {
        return Err("refused non-activating policy; no windows created".into());
    }
    let array = |r: NSRect| [r.origin.x, r.origin.y, r.size.width, r.size.height];
    let mut rows = Vec::new();
    for (name, style) in [
        ("borderless", NSWindowStyleMask::Borderless),
        ("resizable-borderless", NSWindowStyleMask::Resizable),
    ] {
        // Never order this private test window on screen or make it key.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(320.0, 200.0)),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        for delta in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let requested = NSRect::new(
                NSPoint::new(168.0 + delta, 100.0 + delta),
                NSSize::new(320.0, 200.0),
            );
            let screen = window.screen();
            let constrained = window.constrainFrameRect_toScreen(requested, screen.as_deref());
            window.setFrame_display(requested, false);
            let frame = window.frame();
            window.setFrameOrigin(requested.origin);
            let origin = window.frame();
            window.setFrame_display_animate(requested, false, false);
            let animated = window.frame();
            assert!(!window.isVisible() && !window.isKeyWindow());
            rows.push(serde_json::json!({
                "style":name, "requested":array(requested),
                "constrained":array(constrained), "setFrame":array(frame),
                "setFrameOrigin":array(origin), "setFrameNoAnimation":array(animated),
                "backingScale":window.backingScaleFactor(),
                "visible":window.isVisible(), "key":window.isKeyWindow(),
            }));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "scope":"new empty hidden AppKit windows only; no Tauri or on-screen acceptance",
            "activationProhibited":true, "rows":rows,
        }))?
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("AppKit frame probe requires macOS");
}
