//! Test the production observation adapter against a private Foundation
//! notification center. No NSApplication or NSWindow is created or controlled.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "../src/native_window_events.rs"]
mod native_window_events;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use native_window_events::{LayoutEvent, WindowEvents, layout_reaction};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplicationDidChangeScreenParametersNotification, NSWindowDidChangeScreenNotification,
        NSWindowDidEndLiveResizeNotification,
    };
    use objc2_foundation::{NSNotificationCenter, NSObject};
    use std::sync::{Arc, Mutex};
    let mtm = MainThreadMarker::new().ok_or("main thread required")?;
    let center = NSNotificationCenter::new();
    let separate = NSNotificationCenter::new();
    let application = NSObject::new();
    let window = NSObject::new();
    let other_window = NSObject::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let subscribe = || {
        let calls = calls.clone();
        WindowEvents::subscribe(mtm, center.clone(), &application, &window, move |event| {
            assert!(
                MainThreadMarker::new().is_some(),
                "delivery is on the main queue"
            );
            calls.lock().expect("probe capture").push(event);
        })
    };
    let post_matching = || unsafe {
        center.postNotificationName_object(
            NSApplicationDidChangeScreenParametersNotification,
            Some(&application),
        );
        center.postNotificationName_object(NSWindowDidChangeScreenNotification, Some(&window));
        center.postNotificationName_object(NSWindowDidEndLiveResizeNotification, Some(&window));
    };
    let first = subscribe();
    unsafe {
        center.postNotificationName_object(
            NSApplicationDidChangeScreenParametersNotification,
            Some(&other_window),
        );
        center
            .postNotificationName_object(NSWindowDidChangeScreenNotification, Some(&other_window));
        center
            .postNotificationName_object(NSWindowDidEndLiveResizeNotification, Some(&other_window));
        separate.postNotificationName_object(NSWindowDidEndLiveResizeNotification, Some(&window));
    }
    assert!(calls.lock().expect("probe capture").is_empty());
    post_matching();
    let expected = vec![
        LayoutEvent::DisplaysChanged,
        LayoutEvent::WindowScreenChanged,
        LayoutEvent::ResizeEnded,
    ];
    assert_eq!(*calls.lock().expect("probe capture"), expected);
    drop(first);
    post_matching();
    assert_eq!(
        *calls.lock().expect("probe capture"),
        expected,
        "dropping removes every token"
    );
    let second = subscribe();
    post_matching();
    assert_eq!(
        calls.lock().expect("probe capture").len(),
        6,
        "reinstall delivers exactly once per event"
    );
    drop(second);
    post_matching();
    assert_eq!(calls.lock().expect("probe capture").len(), 6);
    let mut guarded = 0;
    for event in [
        LayoutEvent::DisplaysChanged,
        LayoutEvent::WindowScreenChanged,
        LayoutEvent::Resized,
        LayoutEvent::ResizeEnded,
    ] {
        for preview in [false, true] {
            for pending in [false, true] {
                for (visible, hidden, live) in [
                    (false, false, false),
                    (true, true, false),
                    (true, false, true),
                ] {
                    assert!(
                        !layout_reaction(event, visible, hidden, live, preview, pending).reposition
                    );
                    guarded += 1;
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "result":"passed", "nativeWindowsCreated":0, "nativeEndToEnd":false,
            "scope":"production adapter on a private Foundation center with opaque NSObject identities",
            "matchingDeliveries":6, "unexpectedDeliveries":0, "guardedReactions":guarded,
            "checks":["exact app and window identity filters", "main queue delivery", "observer removal", "reinstall without duplicates", "hidden/live policy"],
            "adapterBlake3":blake3::hash(include_bytes!("../src/native_window_events.rs")).to_hex().to_string(),
        }))?
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Foundation notification probe requires macOS");
}
