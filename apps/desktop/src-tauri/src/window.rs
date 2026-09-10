use paste_domain::DesktopPreferences;
use std::sync::Mutex;
use tauri::{LogicalSize, Manager, WebviewWindow};
#[cfg(any(not(target_os = "macos"), test))]
use tauri::{PhysicalPosition, PhysicalSize};

const EXPANDED_HEIGHT: f64 = 248.0;
const COMPACT_HEIGHT: f64 = 148.0;
const FLOATING_GAP_POINTS: f64 = 12.0;

#[derive(Default)]
pub struct PreviewFrame {
    saved: Mutex<Option<LogicalSize<f64>>>,
    dock: Mutex<DockSizeIntent>,
    pending_display: std::sync::atomic::AtomicBool,
}

/// Do not feed the point-grid adjustment back into the user's requested size.
/// Otherwise each change in work-area parity can shrink the panel again.
#[derive(Default)]
struct DockSizeIntent {
    preferred: Option<LogicalSize<f64>>,
    applied: Option<LogicalSize<f64>>,
    user_resizing: bool,
}

impl DockSizeIntent {
    fn resolve(
        &mut self,
        current: LogicalSize<f64>,
        requested: Option<LogicalSize<f64>>,
    ) -> LogicalSize<f64> {
        if let Some(requested) = requested {
            self.preferred = Some(requested);
        } else if self.preferred.is_none() {
            self.preferred = Some(current);
        }
        self.preferred.unwrap_or(current)
    }

    fn observe_user_resize(&mut self, current: LogicalSize<f64>, ended: bool) {
        // A grab without motion must not turn a parity-adjusted width into a
        // new preference. Once moved, however, returning to that width is an
        // intentional user choice and must not retain an intermediate size.
        let changed = self.user_resizing || self.applied != Some(current);
        if changed {
            self.preferred = Some(current);
        }
        self.user_resizing = changed && !ended;
    }
}

fn preferred_dock_size(window: &WebviewWindow) -> tauri::Result<LogicalSize<f64>> {
    if let Some(state) = window.app_handle().try_state::<PreviewFrame>()
        && let Some(preferred) = state
            .dock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .preferred
    {
        return Ok(preferred);
    }
    Ok(window.outer_size()?.to_logical(window.scale_factor()?))
}

fn preview_is_open(window: &WebviewWindow) -> bool {
    window
        .app_handle()
        .try_state::<PreviewFrame>()
        .is_some_and(|state| state.saved.lock().is_ok_and(|saved| saved.is_some()))
}

#[cfg(any(target_os = "macos", test))]
fn should_close_preview(
    key: u16,
    modified: bool,
    owns_focus: bool,
    open: bool,
    native_interaction: bool,
) -> bool {
    key == 53 && !modified && owns_focus && open && !native_interaction
}

#[cfg(target_os = "macos")]
fn native_interaction_owns_escape(mtm: objc2::MainThreadMarker) -> bool {
    use objc2_app_kit::{NSApplication, NSEventTrackingRunLoopMode};
    use objc2_foundation::{NSObjectProtocol, NSRunLoop};
    // Menus and mouse tracking must get their own cancellation first. Sheets
    // and IME composition similarly take precedence over closing the reader.
    let tracking = NSRunLoop::currentRunLoop()
        .currentMode()
        .is_some_and(|mode| &*mode == unsafe { NSEventTrackingRunLoopMode });
    let app = NSApplication::sharedApplication(mtm);
    let key_window = app.keyWindow();
    let sheet = key_window
        .as_ref()
        .is_some_and(|window| window.attachedSheet().is_some());
    let composing = key_window
        .and_then(|window| window.firstResponder())
        .is_some_and(|responder| {
            responder.respondsToSelector(objc2::sel!(hasMarkedText))
                && unsafe { objc2::msg_send![&responder, hasMarkedText] }
        });
    tracking || app.modalWindow().is_some() || sheet || composing
}

/// Invoked on the AppKit thread. Save the dock size only on the first open;
/// switching documents must not overwrite it with the expanded reader size.
pub fn set_preview_open(window: &WebviewWindow, open: bool) -> Result<(), String> {
    if window.label() != "main" {
        return Err("只有主窗口可切换预览布局。".into());
    }
    let state = window.state::<PreviewFrame>();
    let previous = *state.saved.lock().map_err(|e| e.to_string())?;
    if open == previous.is_some() {
        return Ok(());
    }
    if open {
        let size = preferred_dock_size(window).map_err(|e| e.to_string())?;
        *state.saved.lock().map_err(|e| e.to_string())? = Some(size);
        if let Err(error) = position_at_screen_bottom(window) {
            *state.saved.lock().map_err(|e| e.to_string())? = None;
            let _ = position_window(window, Some(size));
            return Err(error.to_string());
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = install_preview_escape(window) {
            let _ = set_preview_open(window, false);
            return Err(error);
        }
    } else if let Some(size) = previous {
        #[cfg(target_os = "macos")]
        remove_preview_escape();
        *state.saved.lock().map_err(|e| e.to_string())? = None;
        if let Err(error) = position_window(window, Some(size)) {
            *state.saved.lock().map_err(|e| e.to_string())? = previous;
            #[cfg(target_os = "macos")]
            let _ = install_preview_escape(window);
            return Err(error.to_string());
        }
    }
    if window.state::<crate::DesktopState>().native_test.is_some() {
        eprintln!("Native UI test preview layout: open={open} saved_dock={previous:?}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
thread_local! {
    static PREVIEW_ESCAPE_MONITOR: std::cell::RefCell<Option<objc2::rc::Retained<objc2::runtime::AnyObject>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
fn remove_preview_escape() {
    PREVIEW_ESCAPE_MONITOR.with(|slot| {
        if let Some(monitor) = slot.borrow_mut().take() {
            // The exact token returned by NSEvent's local monitor API.
            unsafe { objc2_app_kit::NSEvent::removeMonitor(&monitor) };
        }
    });
}

#[cfg(target_os = "macos")]
fn install_preview_escape(window: &WebviewWindow) -> Result<(), String> {
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use tauri::Emitter;
    objc2::MainThreadMarker::new().ok_or("预览快捷键必须在主线程设置。")?;
    let view = unsafe {
        (window.ns_view().map_err(|e| e.to_string())? as *const objc2_app_kit::NSView).as_ref()
    }
    .ok_or("预览窗口已关闭。")?;
    let number = view.window().ok_or("预览窗口已关闭。")?.windowNumber();
    let app = window.app_handle().clone();
    let handler = block2::RcBlock::new(move |pointer: std::ptr::NonNull<NSEvent>| {
        // AppKit calls local monitors on the main thread with a live event.
        let event = unsafe { pointer.as_ref() };
        let modifiers = NSEventModifierFlags::Command
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Shift;
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return pointer.as_ptr();
        };
        let Some(main) = app.get_webview_window("main") else {
            return pointer.as_ptr();
        };
        let owns_focus = event.windowNumber() == number && main.is_focused().unwrap_or(false);
        // Inspect native interaction state only for a candidate close key.
        let open = preview_is_open(&main);
        let modified = event.modifierFlags().intersects(modifiers);
        if !should_close_preview(event.keyCode(), modified, owns_focus, open, false) {
            return pointer.as_ptr();
        }
        let native_interaction = native_interaction_owns_escape(mtm);
        if let Some(profile) = main.state::<crate::DesktopState>().native_test.as_ref()
            && profile.take_trace_budget(1)
        {
            eprintln!("Native UI test preview Escape: native_interaction={native_interaction}");
        }
        if should_close_preview(
            event.keyCode(),
            modified,
            owns_focus,
            open,
            native_interaction,
        ) && main.emit("pasters-close-preview", ()).is_ok()
        {
            // WKWebView's PDF subframe does not bubble key events to the app.
            // Never intercept other windows, ordinary text editing or dragging.
            std::ptr::null_mut()
        } else {
            pointer.as_ptr()
        }
    });
    remove_preview_escape();
    let token = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    }
    .ok_or("无法连接预览关闭快捷键。")?;
    PREVIEW_ESCAPE_MONITOR.with(|slot| *slot.borrow_mut() = Some(token));
    Ok(())
}

pub fn apply_frame_style(window: &WebviewWindow) -> Result<(), String> {
    configure_spaces(window).map_err(|error| error.to_string())?;
    window.set_shadow(false).map_err(|e| e.to_string())?;
    // Install once. Slider changes adjust only web background tint, never the
    // native panel's alpha/frame or the lifetime of its blur layer.
    #[cfg(target_os = "macos")]
    window
        .set_effects(
            tauri::window::EffectsBuilder::new()
                .effect(tauri::window::Effect::HudWindow)
                .state(tauri::window::EffectState::Active)
                .radius(16.0)
                .build(),
        )
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        objc2::MainThreadMarker::new().ok_or("窗口圆角必须在主线程设置。")?;
        let pointer = window.ns_view().map_err(|e| e.to_string())?;
        let view = unsafe { (pointer as *const objc2_app_kit::NSView).as_ref() }
            .ok_or("窗口视图已关闭。")?;
        view.setWantsLayer(true);
        let layer = view.layer().ok_or("无法创建窗口圆角层。")?;
        layer.setCornerRadius(16.0);
        layer.setMasksToBounds(true);
        if let Some(native) = view.window() {
            native.setHasShadow(false);
            native.invalidateShadow();
        }
        if window
            .app_handle()
            .try_state::<crate::DesktopState>()
            .is_some_and(|s| s.native_test.is_some())
        {
            eprintln!(
                "Native UI test frame style: radius={} masks={} shadow={}",
                layer.cornerRadius(),
                layer.masksToBounds(),
                view.window().is_some_and(|w| w.hasShadow())
            );
        }
    }
    Ok(())
}

pub fn position_at_screen_bottom(window: &WebviewWindow) -> tauri::Result<()> {
    position_window(window, None)
}

/// Size and destination belong to one layout request. In particular, closing
/// the reader must not query its old size after scheduling a separate resize.
fn position_window(
    window: &WebviewWindow,
    requested: Option<LogicalSize<f64>>,
) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if objc2::MainThreadMarker::new().is_some() {
            return apply_native_frame(window, requested);
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let main = window.clone();
        window.run_on_main_thread(move || {
            let _ = sender.send(apply_native_frame(&main, requested));
        })?;
        // The main-thread branch above never waits on its own queue.
        receiver
            .recv()
            .map_err(|error| std::io::Error::other(error.to_string()))?
    }
    #[cfg(not(target_os = "macos"))]
    position_portable_window(window, requested)
}

#[cfg(not(target_os = "macos"))]
fn position_portable_window(
    window: &WebviewWindow,
    requested: Option<LogicalSize<f64>>,
) -> tauri::Result<()> {
    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .or_else(|| window.available_monitors().ok()?.into_iter().next());
    let Some(monitor) = monitor else {
        return Ok(());
    };
    // Both work_area and outer_size are physical pixels. Scaling again here
    // would shift the panel on Retina or mixed-scale secondary displays.
    let work_area = monitor.work_area();
    let (origin, available) = if work_area.size.width > 0 && work_area.size.height > 0 {
        (work_area.position, work_area.size)
    } else {
        (*monitor.position(), *monitor.size())
    };
    let current_size = window.outer_size()?;
    let gap = (FLOATING_GAP_POINTS * monitor.scale_factor())
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32;
    let bounds = if preview_is_open(window) {
        reader_bounds(origin, available, monitor.scale_factor())
    } else {
        bottom_centered_bounds(
            origin,
            available,
            requested.map_or(current_size, |size| {
                size.to_physical(monitor.scale_factor())
            }),
            gap,
        )
    };
    let Some((position, size)) = bounds else {
        return Ok(());
    };
    if size != current_size {
        window.set_size(size)?;
    }
    window.set_position(position)?;
    if window
        .app_handle()
        .try_state::<crate::DesktopState>()
        .is_some_and(|state| state.native_test.is_some())
    {
        eprintln!(
            "Native UI test geometry request: area={origin:?}/{available:?} requested={position:?}/{size:?} observed_before_async_move={:?}/{:?} scale={}",
            window.outer_position()?,
            window.outer_size()?,
            monitor.scale_factor()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
thread_local! {
    static APPLYING_FRAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static WINDOW_EVENTS: std::cell::RefCell<Option<crate::native_window_events::WindowEvents>> = const { std::cell::RefCell::new(None) };
    static SPACE_EVENTS: std::cell::RefCell<Option<crate::native_space_events::ActiveSpaceEvents>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
fn install_native_positioning(window: &WebviewWindow) -> Result<(), String> {
    use objc2_app_kit::{NSApplication, NSWindow};
    use objc2_foundation::NSNotificationCenter;
    let mtm = objc2::MainThreadMarker::new().ok_or("窗口监听必须在主线程设置。")?;
    let native =
        unsafe { (window.ns_window().map_err(|e| e.to_string())? as *const NSWindow).as_ref() }
            .ok_or("窗口已关闭。")?;
    let application = NSApplication::sharedApplication(mtm);
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    let observers = crate::native_window_events::WindowEvents::subscribe(
        mtm,
        NSNotificationCenter::defaultCenter(),
        &application,
        native,
        move |event| {
            if let Some(window) = app.get_webview_window(&label) {
                handle_native_layout_event(&window, event);
            }
        },
    );
    let previous = WINDOW_EVENTS.with(|slot| slot.replace(Some(observers)));
    drop(previous);
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    let observer = crate::native_space_events::ActiveSpaceEvents::subscribe(mtm, move || {
        let Some(window) = app.get_webview_window(&label) else {
            return;
        };
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let Ok(pointer) = window.ns_window() else {
            return;
        };
        let Some(native) = (unsafe { (pointer as *const NSWindow).as_ref() }) else {
            return;
        };
        trace_invocation(native, mtm, "space-changed");
        if should_dismiss_for_space_change(
            native.isVisible(),
            native.inLiveResize(),
            native_interaction_owns_escape(mtm),
        ) {
            let _ = hide_main_window(&window);
        }
    });
    drop(SPACE_EVENTS.with(|slot| slot.replace(Some(observer))));
    Ok(())
}

#[cfg(target_os = "macos")]
fn handle_native_layout_event(
    window: &WebviewWindow,
    event: crate::native_window_events::LayoutEvent,
) {
    use crate::native_window_events::{LayoutEvent, layout_reaction};
    use objc2_app_kit::{NSApplication, NSWindow};
    use std::sync::atomic::Ordering;
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    let Some(native) = (unsafe { (pointer as *const NSWindow).as_ref() }) else {
        return;
    };
    let preview = preview_is_open(window);
    let state = window.state::<PreviewFrame>();
    let pending = if event == LayoutEvent::ResizeEnded {
        state.pending_display.swap(false, Ordering::Relaxed)
    } else {
        state.pending_display.load(Ordering::Relaxed)
    };
    let reaction = layout_reaction(
        event,
        native.isVisible(),
        NSApplication::sharedApplication(mtm).isHidden(),
        native.inLiveResize(),
        preview,
        pending,
    );
    if reaction.defer_display {
        state.pending_display.store(true, Ordering::Relaxed);
    }
    if reaction.remember_user_size
        || (event == LayoutEvent::Resized && native.inLiveResize() && !preview)
    {
        let size = native.frame().size;
        state
            .dock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .observe_user_resize(
                LogicalSize::new(size.width, size.height),
                reaction.remember_user_size,
            );
    }
    // Never call show_main_window, show, set_focus or orderFront here.
    if reaction.reposition
        && let Err(error) = position_at_screen_bottom(window)
    {
        eprintln!("Window display layout failed: {error}");
    }
    if let Some(profile) = window.state::<crate::DesktopState>().native_test.as_ref()
        && profile.take_trace_budget(1)
    {
        eprintln!("Native UI test display notification: event={event:?} reaction={reaction:?}");
    }
}

#[cfg(target_os = "macos")]
fn apply_native_frame(
    window: &WebviewWindow,
    requested: Option<LogicalSize<f64>>,
) -> tauri::Result<()> {
    use objc2_app_kit::{NSScreen, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let mtm = objc2::MainThreadMarker::new()
        .ok_or_else(|| std::io::Error::other("窗口布局必须在主线程更新。"))?;
    if APPLYING_FRAME.with(|active| active.get()) {
        return Ok(());
    }
    // Use the selected NSScreen's logical frame directly. Multiplying global
    // screen origins by one display's scale breaks mixed-scale arrangements.
    let pointer = window.ns_window()?;
    let native = unsafe { (pointer as *const NSWindow).as_ref() }
        .ok_or_else(|| std::io::Error::other("窗口已关闭。"))?;
    let Some(screen) = native
        .screen()
        .or_else(|| NSScreen::mainScreen(mtm))
        .or_else(|| NSScreen::screens(mtm).firstObject())
    else {
        return Ok(());
    };
    let visible = screen.visibleFrame();
    let current = native.frame();
    let preview = preview_is_open(window);
    let state = window.state::<PreviewFrame>();
    let current_size = LogicalSize::new(current.size.width, current.size.height);
    if native.inLiveResize() {
        if !preview {
            state
                .dock
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .observe_user_resize(current_size, false);
        }
        return Ok(());
    }
    let requested = if preview {
        state
            .saved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(current_size)
    } else {
        state
            .dock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(current_size, requested)
    };
    let Some([x, y, width, height]) = cocoa_frame_bounds(
        [
            visible.origin.x,
            visible.origin.y,
            visible.size.width,
            visible.size.height,
        ],
        requested,
        screen.backingScaleFactor(),
        preview,
    ) else {
        return Err(std::io::Error::other("屏幕工作区或窗口尺寸无效。").into());
    };
    let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
    state
        .pending_display
        .store(false, std::sync::atomic::Ordering::Relaxed);
    if frame == current {
        crate::workspace::position(window.app_handle())?;
        if !preview {
            state.dock.lock().unwrap_or_else(|e| e.into_inner()).applied = Some(current_size);
        }
        return Ok(());
    }
    struct ResetFrameGuard;
    impl Drop for ResetFrameGuard {
        fn drop(&mut self) {
            APPLYING_FRAME.with(|active| active.set(false));
        }
    }
    APPLYING_FRAME.with(|active| active.set(true));
    let _guard = ResetFrameGuard;
    // One AppKit update: no queued setContentSize/setFrameTopLeftPoint pair,
    // no intermediate frame, no activation, no system preference changes.
    native.setFrame_display(frame, true);
    crate::workspace::position(window.app_handle())?;
    if !preview {
        let applied = native.frame().size;
        state.dock.lock().unwrap_or_else(|e| e.into_inner()).applied =
            Some(LogicalSize::new(applied.width, applied.height));
    }
    if window
        .app_handle()
        .try_state::<crate::DesktopState>()
        .is_some_and(|state| state.native_test.is_some())
    {
        eprintln!(
            "Native UI test atomic frame: preview={preview} previous={current:?} requested={frame:?} applied={:?}",
            native.frame()
        );
        eprintln!(
            "Native UI test frame intent: preview={preview} preferred_width={} preferred_height={} grid=whole-point-inward",
            requested.width, requested.height
        );
        trace_applied_native_geometry(window);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn cocoa_frame_bounds(
    visible: [f64; 4],
    requested: LogicalSize<f64>,
    scale: f64,
    preview: bool,
) -> Option<[f64; 4]> {
    let [x, y, width, height] = visible;
    if visible.iter().any(|v| !v.is_finite())
        || width <= 0.0
        || height <= 0.0
        || !scale.is_finite()
        || scale <= 0.0
        || !requested.width.is_finite()
        || !requested.height.is_finite()
        || requested.width <= 0.0
        || requested.height <= 0.0
        || width * scale > u32::MAX as f64
        || height * scale > u32::MAX as f64
    {
        return None;
    }
    // AppKit quantizes NSWindow origins to whole logical points, even on a
    // Retina display (see the isolated probe_window_frames example). Align
    // both edges inward around the real center instead of asking AppKit to
    // floor a half-point origin and silently unbalance the margins. The
    // viewport, not the cards or typography, can lose at most 1 pt for the
    // ordinary integral NSScreen frames and integral preferred sizes.
    let (wanted_width, wanted_height) = if preview {
        (
            requested.width.min((width - 24.0).max(1.0)),
            (requested.height + 360.0).min((height - 24.0).max(1.0)),
        )
    } else {
        let side_gap = FLOATING_GAP_POINTS.min(((width - 1.0) / 2.0).max(0.0));
        let bottom_gap = FLOATING_GAP_POINTS.min((height - 1.0).max(0.0));
        (
            requested.width.min(width - side_gap * 2.0),
            requested.height.min(height - bottom_gap),
        )
    };
    let left = (x + (width - wanted_width) / 2.0).ceil();
    let right = (x + (width + wanted_width) / 2.0).floor();
    let bottom = (y + FLOATING_GAP_POINTS.min((height - 1.0).max(0.0))).ceil();
    let top = (bottom + wanted_height).min(y + height).floor();
    (right > left && top > bottom).then_some([left, bottom, right - left, top - bottom])
}

#[cfg(any(not(target_os = "macos"), test))]
fn reader_bounds(
    origin: PhysicalPosition<i32>,
    available: PhysicalSize<u32>,
    scale: f64,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    if available.width == 0 || available.height == 0 || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let pixels = |points: f64| (points * scale).round().clamp(1.0, u32::MAX as f64) as u32;
    let margin = pixels(24.0);
    let size = PhysicalSize::new(
        pixels(960.0).min(
            available
                .width
                .saturating_sub(margin.saturating_mul(2))
                .max(1),
        ),
        pixels(760.0).min(
            available
                .height
                .saturating_sub(margin.saturating_mul(2))
                .max(1),
        ),
    );
    let center = |start: i32, space: u32, length: u32| {
        (i64::from(start) + i64::from(space - length) / 2)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    Some((
        PhysicalPosition::new(
            center(origin.x, available.width, size.width),
            center(origin.y, available.height, size.height),
        ),
        size,
    ))
}

#[cfg(any(not(target_os = "macos"), test))]
fn bottom_centered_bounds(
    origin: PhysicalPosition<i32>,
    available: PhysicalSize<u32>,
    requested: PhysicalSize<u32>,
    gap: u32,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    if available.width == 0 || available.height == 0 {
        return None;
    }
    let side_gap = gap.min((available.width - 1) / 2);
    let bottom_gap = gap.min(available.height - 1);
    let size = PhysicalSize::new(
        requested.width.max(1).min(available.width - side_gap * 2),
        requested.height.max(1).min(available.height - bottom_gap),
    );
    let x = i64::from(origin.x) + i64::from(available.width - size.width) / 2;
    let y = i64::from(origin.y) + i64::from(available.height - size.height - bottom_gap);
    let clamp = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    Some((PhysicalPosition::new(clamp(x), clamp(y)), size))
}

pub fn install_positioning(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("只有主窗口可安装底部定位监听。".into());
    }
    #[cfg(target_os = "macos")]
    install_native_positioning(window)?;
    let panel = window.clone();
    window.on_window_event(move |event| {
        #[cfg(target_os = "macos")]
        if matches!(event, tauri::WindowEvent::Destroyed) {
            remove_preview_escape();
            let observers = WINDOW_EVENTS.with(|slot| slot.take());
            drop(observers);
            drop(SPACE_EVENTS.with(|slot| slot.take()));
        }
        // Keep event evidence alongside the synchronous native frame read.
        if let tauri::WindowEvent::Moved(position) = event
            && panel
                .app_handle()
                .try_state::<crate::DesktopState>()
                .is_some_and(|state| state.native_test.is_some())
        {
            eprintln!(
                "Native UI test geometry applied: position={position:?} size={:?} work_area={:?}",
                panel.outer_size(),
                panel
                    .current_monitor()
                    .map(|monitor| monitor.map(|monitor| *monitor.work_area()))
            );
            #[cfg(target_os = "macos")]
            trace_applied_native_geometry(&panel);
        }
        #[cfg(target_os = "macos")]
        match event {
            tauri::WindowEvent::ScaleFactorChanged { .. } => handle_native_layout_event(
                &panel,
                crate::native_window_events::LayoutEvent::WindowScreenChanged,
            ),
            tauri::WindowEvent::Resized(_) => handle_native_layout_event(
                &panel,
                crate::native_window_events::LayoutEvent::Resized,
            ),
            _ => {}
        }
        #[cfg(not(target_os = "macos"))]
        if matches!(event, tauri::WindowEvent::ScaleFactorChanged { .. })
            || (matches!(event, tauri::WindowEvent::Resized(_)) && !preview_is_open(&panel))
        {
            let _ = position_at_screen_bottom(&panel);
        }
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn trace_applied_native_geometry(window: &WebviewWindow) {
    // Called only for isolated Moved events. Read the applied AppKit frame in
    // logical points, independently of Tao's physical-coordinate conversion.
    // No correction or diagnostic window is created here.
    let Some(_main_thread) = objc2::MainThreadMarker::new() else {
        eprintln!("Native UI test AppKit geometry unavailable: not on main thread");
        return;
    };
    let Ok(pointer) = window.ns_view() else {
        return;
    };
    let Some(view) = (unsafe { (pointer as *const objc2_app_kit::NSView).as_ref() }) else {
        return;
    };
    let Some(native) = view.window() else {
        return;
    };
    let Some(screen) = native.screen() else {
        return;
    };
    let frame = native.frame();
    let visible = screen.visibleFrame();
    let left = frame.origin.x - visible.origin.x;
    let right = visible.origin.x + visible.size.width - frame.origin.x - frame.size.width;
    let bottom = frame.origin.y - visible.origin.y;
    eprintln!(
        "Native UI test AppKit geometry: frame_points={frame:?} visible_points={visible:?} screen_points={:?} scale={} left_points={left} right_points={right} bottom_points={bottom} center_error_points={}",
        screen.frame(),
        native.backingScaleFactor(),
        (left - right) / 2.0,
    );
}

#[cfg(target_os = "macos")]
fn invocation_collection_behavior(
    mut current: objc2_app_kit::NSWindowCollectionBehavior,
) -> objc2_app_kit::NSWindowCollectionBehavior {
    use objc2_app_kit::NSWindowCollectionBehavior as Behavior;
    // MoveToActiveSpace alone does not give a reused background window
    // membership in the caller's Space before application activation. This is
    // paired with accessory activation and ordering before focus below.
    // The workspace observer dismisses the panel when the user changes Space.
    current.remove(
        Behavior::MoveToActiveSpace | Behavior::FullScreenPrimary | Behavior::FullScreenNone,
    );
    current.insert(Behavior::CanJoinAllSpaces | Behavior::FullScreenAuxiliary);
    current
}

pub(crate) fn configure_spaces(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        objc2::MainThreadMarker::new()
            .ok_or_else(|| std::io::Error::other("桌面唤起必须在主线程设置。"))?;
        let native = unsafe { (window.ns_window()? as *const objc2_app_kit::NSWindow).as_ref() }
            .ok_or_else(|| std::io::Error::other("窗口已关闭。"))?;
        native.setCollectionBehavior(invocation_collection_behavior(native.collectionBehavior()));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    Ok(())
}

fn on_active_space(window: &WebviewWindow) -> tauri::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        objc2::MainThreadMarker::new()
            .ok_or_else(|| std::io::Error::other("桌面状态必须在主线程读取。"))?;
        let native = unsafe { (window.ns_window()? as *const objc2_app_kit::NSWindow).as_ref() }
            .ok_or_else(|| std::io::Error::other("窗口已关闭。"))?;
        Ok(native.isOnActiveSpace())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(true)
    }
}

fn should_toggle_hide(
    visible: bool,
    focused: bool,
    active_space: bool,
    application_active: bool,
    nonactivating_panel: bool,
) -> bool {
    visible && focused && active_space && (application_active || nonactivating_panel)
}

#[cfg(any(target_os = "macos", test))]
fn should_dismiss_for_space_change(
    visible: bool,
    live_resize: bool,
    native_interaction: bool,
) -> bool {
    visible && !live_resize && !native_interaction
}

fn application_is_active() -> tauri::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let mtm = objc2::MainThreadMarker::new()
            .ok_or_else(|| std::io::Error::other("桌面状态必须在主线程读取。"))?;
        Ok(objc2_app_kit::NSApplication::sharedApplication(mtm).isActive())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

/// Debug builds retain a small local trace of our own window flags only. No
/// clipboard content, window titles, other apps, or desktop identifiers enter it.
#[cfg(target_os = "macos")]
pub(crate) fn trace_invocation(
    native: &objc2_app_kit::NSWindow,
    mtm: objc2::MainThreadMarker,
    stage: &'static str,
) {
    #[cfg(debug_assertions)]
    {
        use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
        thread_local! {
            static TRACE: std::cell::RefCell<Option<(std::fs::File, usize)>> =
                std::cell::RefCell::new(OpenOptions::new().write(true).create_new(true)
                    .mode(0o600).open(std::env::temp_dir().join(format!(
                        "copyrail-window-{}.jsonl", std::process::id()
                    ))).ok().map(|file| (file, 0)));
        }
        let application = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let record = serde_json::json!({
            "stage": stage,
            "frame": [native.frame().origin.x, native.frame().origin.y, native.frame().size.width, native.frame().size.height],
            "visible": native.isVisible(),
            "onActiveSpace": native.isOnActiveSpace(),
            "key": native.isKeyWindow(),
            "applicationActive": application.isActive(),
            "accessory": application.activationPolicy()
                == objc2_app_kit::NSApplicationActivationPolicy::Accessory,
            "collectionBehavior": native.collectionBehavior().0,
            "nonactivatingPanel": native.styleMask().contains(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel),
        });
        TRACE.with(|slot| {
            let mut trace = slot.borrow_mut();
            let Some((file, count)) = trace.as_mut() else {
                return;
            };
            // Bound the debug trace even when a local build runs for days.
            if *count >= 128 {
                use std::io::Seek;
                if file.set_len(0).and_then(|()| file.rewind()).is_err() {
                    return;
                }
                *count = 0;
            }
            let _ = writeln!(file, "{record}");
            *count += 1;
        });
    }
    #[cfg(not(debug_assertions))]
    let _ = (native, mtm, stage);
}

pub fn show_main_window(window: &WebviewWindow) -> tauri::Result<()> {
    // Every user invocation (shortcut, tray, Window menu, reopen, IPC) goes
    // through here. Capture before activating our own window; unknown/self
    // replaces the old target with None rather than reusing stale intent.
    let state = window.state::<crate::DesktopState>();
    if state.native_test.is_none() {
        state
            .paste_target
            .remember_frontmost_application()
            .map_err(std::io::Error::other)?;
    }
    configure_spaces(window)?;
    position_at_screen_bottom(window)?;
    present_on_current_space(window)
}

pub(crate) fn present_on_current_space(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindow};
        let mtm = objc2::MainThreadMarker::new()
            .ok_or_else(|| std::io::Error::other("窗口唤起必须在主线程执行。"))?;
        let application = NSApplication::sharedApplication(mtm);
        // Keep the utility policy effective even after a framework reopen or
        // other native window changes. Setting policy does not activate it.
        if application.activationPolicy() != NSApplicationActivationPolicy::Accessory
            && !application.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
        {
            return Err(std::io::Error::other("无法设置原生面板的应用模式。").into());
        }
        let native = unsafe { (window.ns_window()? as *const NSWindow).as_ref() }
            .ok_or_else(|| std::io::Error::other("窗口已关闭。"))?;
        trace_invocation(native, mtm, "before-order");
        // A nonactivating NSPanel takes keyboard focus without activating the
        // application. Never call Tauri set_focus / NSApplication activate here:
        // either would reintroduce application-driven Space switching.
        native.orderFrontRegardless();
        trace_invocation(native, mtm, "after-order");
        native.makeKeyWindow();
        trace_invocation(native, mtm, "after-focus");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.show()?;
        window.set_focus()
    }
}

pub fn hide_main_window(window: &WebviewWindow) -> tauri::Result<()> {
    crate::workspace::dismiss(window.app_handle(), false)?;
    window
        .state::<crate::DesktopState>()
        .paste_target
        .invalidate()
        .map_err(std::io::Error::other)?;
    window.hide()?;
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new()
        && let Some(native) =
            unsafe { (window.ns_window()? as *const objc2_app_kit::NSWindow).as_ref() }
    {
        trace_invocation(native, mtm, "hidden");
    }
    Ok(())
}

pub fn apply_desktop_preferences(
    window: &WebviewWindow,
    preferences: DesktopPreferences,
) -> tauri::Result<()> {
    window.set_content_protected(preferences.screen_share_protection)?;
    if let Some(aux) = window.app_handle().get_webview_window("workspace") {
        aux.set_content_protected(preferences.screen_share_protection)?;
    }
    if let Some(state) = window.app_handle().try_state::<PreviewFrame>() {
        let mut saved = state.saved.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(size) = saved.as_mut() {
            size.height = if preferences.compact_mode {
                COMPACT_HEIGHT
            } else {
                EXPANDED_HEIGHT
            };
            return Ok(());
        }
    }
    let current_size = preferred_dock_size(window)?;
    let logical_height = if preferences.compact_mode {
        COMPACT_HEIGHT
    } else {
        EXPANDED_HEIGHT
    };
    position_window(
        window,
        Some(LogicalSize::new(current_size.width, logical_height)),
    )
}

pub fn toggle_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    if should_toggle_hide(
        window.is_visible()?,
        window.is_focused()?
            || app
                .get_webview_window("workspace")
                .is_some_and(|w| w.is_focused().unwrap_or(false)),
        on_active_space(&window)?,
        application_is_active()?,
        cfg!(target_os = "macos"),
    ) {
        hide_main_window(&window)
    } else {
        show_main_window(&window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonactivating_panel_toggles_while_its_application_stays_inactive() {
        assert!(should_toggle_hide(true, true, true, false, true));
        assert!(!should_toggle_hide(true, false, true, false, true));
        assert!(!should_toggle_hide(false, true, true, false, true));
        assert!(!should_toggle_hide(true, true, false, false, true));
        assert!(!should_toggle_hide(true, true, true, false, false));
    }

    #[test]
    fn shortcut_only_hides_a_focused_window_on_the_current_space() {
        for visible in [false, true] {
            for focused in [false, true] {
                assert!(!should_toggle_hide(visible, focused, false, true, false));
                assert!(!should_toggle_hide(visible, focused, true, false, false));
                assert_eq!(
                    should_toggle_hide(visible, focused, true, true, false),
                    visible && focused
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn invocation_can_join_the_callers_space_without_conflicting_collection_flags() {
        use objc2_app_kit::NSWindowCollectionBehavior as Behavior;
        let original =
            Behavior::MoveToActiveSpace | Behavior::FullScreenPrimary | Behavior::Transient;
        let actual = invocation_collection_behavior(original);
        assert!(actual.contains(Behavior::CanJoinAllSpaces | Behavior::FullScreenAuxiliary));
        assert!(actual.contains(Behavior::Transient));
        assert!(!actual.intersects(
            Behavior::MoveToActiveSpace | Behavior::FullScreenPrimary | Behavior::FullScreenNone
        ));
        assert_eq!(invocation_collection_behavior(actual), actual);
    }

    #[test]
    fn space_changes_dismiss_the_panel_without_interrupting_native_gestures() {
        assert!(should_dismiss_for_space_change(true, false, false));
        assert!(!should_dismiss_for_space_change(false, false, false));
        assert!(!should_dismiss_for_space_change(true, true, false));
        assert!(!should_dismiss_for_space_change(true, false, true));
    }

    #[test]
    fn cocoa_retina_frame_centers_on_the_appkit_point_grid() {
        let visible = [49.0, 0.0, 1679.0, 1084.0];
        let dock = LogicalSize::new(1440.0, 248.0);
        assert_eq!(
            cocoa_frame_bounds(visible, dock, 2.0, false),
            Some([169.0, 12.0, 1439.0, 248.0])
        );
        assert_eq!(
            cocoa_frame_bounds(visible, dock, 2.0, true),
            Some([169.0, 12.0, 1439.0, 608.0])
        );
    }

    #[test]
    fn cocoa_restore_uses_saved_dock_size_not_the_readers_current_size() {
        let visible = [49.0, 0.0, 1679.0, 1084.0];
        let saved = LogicalSize::new(1440.0, 248.0);
        let reader = LogicalSize::new(960.0, 760.0);
        let restored = cocoa_frame_bounds(visible, saved, 2.0, false).expect("saved dock");
        assert_eq!(restored, [169.0, 12.0, 1439.0, 248.0]);
        assert_ne!(
            Some(restored),
            cocoa_frame_bounds(visible, reader, 2.0, false)
        );
        assert_eq!(
            cocoa_frame_bounds(visible, LogicalSize::new(1440.0, 148.0), 2.0, false),
            Some([169.0, 12.0, 1439.0, 148.0])
        );
    }

    #[test]
    fn cocoa_secondary_screen_origin_is_not_rescaled() {
        let visible = [-1920.0, -180.0, 1920.0, 1040.0];
        for scale in [1.0, 2.0] {
            assert_eq!(
                cocoa_frame_bounds(visible, LogicalSize::new(1440.0, 248.0), scale, false),
                Some([-1680.0, -168.0, 1440.0, 248.0])
            );
            assert_eq!(
                cocoa_frame_bounds(visible, LogicalSize::new(1440.0, 248.0), scale, true),
                Some([-1680.0, -168.0, 1440.0, 608.0])
            );
        }
    }

    #[test]
    fn cocoa_bounds_clamp_small_displays_and_reject_invalid_geometry() {
        let dock = LogicalSize::new(1440.0, 248.0);
        assert_eq!(
            cocoa_frame_bounds([0.0, 0.0, 640.0, 480.0], dock, 1.0, true),
            Some([12.0, 12.0, 616.0, 456.0])
        );
        assert_eq!(
            cocoa_frame_bounds([0.0, 0.0, 640.0, 480.0], dock, 1.0, false),
            Some([12.0, 12.0, 616.0, 248.0])
        );
        assert_eq!(
            cocoa_frame_bounds([0.0, 0.0, 0.5, 0.5], dock, 2.0, false),
            None
        );
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(cocoa_frame_bounds([0.0, 0.0, 640.0, 480.0], dock, invalid, false).is_none());
            assert!(cocoa_frame_bounds([0.0, 0.0, invalid, 480.0], dock, 2.0, false).is_none());
            assert!(
                cocoa_frame_bounds(
                    [0.0, 0.0, 640.0, 480.0],
                    LogicalSize::new(invalid, 248.0),
                    2.0,
                    false
                )
                .is_none()
            );
        }
        assert!(cocoa_frame_bounds([f64::NAN, 0.0, 640.0, 480.0], dock, 2.0, false).is_none());
        assert!(cocoa_frame_bounds([0.0, 0.0, f64::MAX, 480.0], dock, 2.0, false).is_none());
    }

    #[test]
    fn cocoa_integral_work_areas_have_equal_margins_without_scaling_content() {
        for scale in [1.0, 2.0] {
            for x in [-1921.0, -1920.0, 0.0, 49.0, 50.0] {
                for y in [-181.0, 0.0, 31.0] {
                    for width in [1677.0, 1678.0, 1679.0, 1680.0] {
                        for height in [1083.0, 1084.0] {
                            for preview in [false, true] {
                                let frame = cocoa_frame_bounds(
                                    [x, y, width, height],
                                    LogicalSize::new(1440.0, 248.0),
                                    scale,
                                    preview,
                                )
                                .expect("valid AppKit work area");
                                let [left, bottom, w, h] = frame;
                                assert!(frame.iter().all(|v| v.fract() == 0.0));
                                assert_eq!(left - x, x + width - left - w);
                                let preferred_width = 1440.0;
                                assert!(
                                    (preferred_width - w) >= 0.0 && (preferred_width - w) <= 1.0
                                );
                                if preview {
                                    assert_eq!(bottom - y, 12.0);
                                    assert_eq!(h, 608.0);
                                } else {
                                    assert_eq!(bottom - y, 12.0);
                                    assert_eq!(h, 248.0);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dock_intent_does_not_accumulate_grid_shrinkage_or_small_screen_clamps() {
        let preferred = LogicalSize::new(1440.0, 248.0);
        let mut intent = DockSizeIntent::default();
        let mut current = preferred;
        for i in 0..120 {
            let width = [1679.0, 1680.0, 640.0][i % 3];
            let requested = intent.resolve(current, None);
            assert_eq!(requested, preferred);
            let frame = cocoa_frame_bounds([0.0, 0.0, width, 1084.0], requested, 2.0, false)
                .expect("display change");
            current = LogicalSize::new(frame[2], frame[3]);
            intent.applied = Some(current);
        }
        assert_eq!(intent.preferred, Some(preferred));
    }

    #[test]
    fn dock_intent_ignores_system_clamps_without_a_user_resize() {
        let preferred = LogicalSize::new(1440.0, 248.0);
        let mut intent = DockSizeIntent {
            preferred: Some(preferred),
            applied: Some(LogicalSize::new(1439.0, 248.0)),
            ..Default::default()
        };
        // AppKit can constrain the frame before the app handles a display
        // notification; it is not evidence that the user requested 616 pt.
        assert_eq!(
            intent.resolve(LogicalSize::new(616.0, 248.0), None),
            preferred
        );
    }

    #[test]
    fn dock_intent_distinguishes_user_resize_and_explicit_preview_restore() {
        let preferred = LogicalSize::new(1440.0, 248.0);
        let applied = LogicalSize::new(1439.0, 248.0);
        let mut intent = DockSizeIntent {
            preferred: Some(preferred),
            applied: Some(applied),
            ..Default::default()
        };
        let reader = LogicalSize::new(959.0, 760.0);
        assert_eq!(intent.resolve(reader, Some(preferred)), preferred);
        let resized = LogicalSize::new(1320.0, 248.0);
        intent.observe_user_resize(resized, true);
        assert_eq!(intent.resolve(resized, None), resized);
        intent.applied = Some(LogicalSize::new(1319.0, 248.0));
        assert_eq!(
            intent.resolve(LogicalSize::new(1319.0, 248.0), None),
            resized
        );
        let compact = LogicalSize::new(1320.0, 148.0);
        assert_eq!(intent.resolve(applied, Some(compact)), compact);
    }

    #[test]
    fn live_resize_without_motion_preserves_intent_and_returning_motion_uses_final_size() {
        let preferred = LogicalSize::new(1440.0, 248.0);
        let applied = LogicalSize::new(1439.0, 248.0);
        let mut intent = DockSizeIntent {
            preferred: Some(preferred),
            applied: Some(applied),
            ..Default::default()
        };
        intent.observe_user_resize(applied, false);
        intent.observe_user_resize(applied, true);
        assert_eq!(intent.preferred, Some(preferred));
        intent.observe_user_resize(LogicalSize::new(1318.0, 248.0), false);
        intent.observe_user_resize(applied, true);
        assert_eq!(intent.preferred, Some(applied));
        assert!(!intent.user_resizing);
    }

    #[test]
    fn preview_escape_preserves_menus_sheets_composition_and_other_windows() {
        assert!(should_close_preview(53, false, true, true, false));
        assert!(!should_close_preview(53, false, true, true, true));
        assert!(!should_close_preview(53, false, false, true, false));
        assert!(!should_close_preview(53, false, true, false, false));
        assert!(!should_close_preview(53, true, true, true, false));
        for key in [0, 36, 49, 51, 123, 124] {
            assert!(!should_close_preview(key, false, true, true, false));
        }
    }

    #[test]
    fn reader_uses_work_area_center_and_logical_dimensions() {
        let (p, s) = reader_bounds(
            PhysicalPosition::new(102, 66),
            PhysicalSize::new(3354, 2168),
            2.0,
        )
        .expect("Retina reader");
        assert_eq!(s, PhysicalSize::new(1920, 1520));
        assert_eq!(p, PhysicalPosition::new(819, 390));
        let (p, s) = reader_bounds(
            PhysicalPosition::new(-1440, 40),
            PhysicalSize::new(1440, 860),
            1.0,
        )
        .expect("secondary reader");
        assert_eq!(s, PhysicalSize::new(960, 760));
        assert_eq!(p, PhysicalPosition::new(-1200, 90));
    }

    #[test]
    fn reader_fits_small_displays_and_rejects_invalid_scales() {
        let (p, s) = reader_bounds(
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(640, 480),
            1.0,
        )
        .expect("small reader");
        assert_eq!(s, PhysicalSize::new(592, 432));
        assert_eq!(p, PhysicalPosition::new(24, 24));
        assert_eq!(
            reader_bounds(PhysicalPosition::new(0, 0), PhysicalSize::new(1, 1), 2.0)
                .expect("tiny reader")
                .1,
            PhysicalSize::new(1, 1)
        );
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                reader_bounds(
                    PhysicalPosition::new(0, 0),
                    PhysicalSize::new(640, 480),
                    scale
                )
                .is_none()
            );
        }
    }

    #[test]
    fn compact_and_tiny_displays_keep_a_bounded_floating_gap() {
        let (p, s) = bottom_centered_bounds(
            PhysicalPosition::new(102, 66),
            PhysicalSize::new(3354, 2168),
            PhysicalSize::new(2880, 296),
            24,
        )
        .expect("compact");
        assert_eq!(p, PhysicalPosition::new(339, 1914));
        assert_eq!(2234 - p.y - s.height as i32, 24);
        let (p, s) = bottom_centered_bounds(
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1, 1),
            PhysicalSize::new(2880, 496),
            24,
        )
        .expect("tiny");
        assert_eq!(p, PhysicalPosition::new(0, 0));
        assert_eq!(s, PhysicalSize::new(1, 1));
    }

    #[test]
    fn retina_panel_is_centered_in_physical_pixels_without_double_scaling() {
        let (position, size) = bottom_centered_bounds(
            PhysicalPosition::new(0, 50),
            PhysicalSize::new(3312, 2078),
            PhysicalSize::new(2880, 496),
            24,
        )
        .expect("visible display");
        assert_eq!(position, PhysicalPosition::new(216, 1608));
        assert_eq!(size, PhysicalSize::new(2880, 496));
    }

    #[test]
    fn secondary_display_negative_origin_and_reserved_work_area_are_respected() {
        let (position, _) = bottom_centered_bounds(
            PhysicalPosition::new(-2560, 240),
            PhysicalSize::new(2560, 1440),
            PhysicalSize::new(1440, 248),
            12,
        )
        .expect("secondary display");
        assert_eq!(position, PhysicalPosition::new(-2000, 1420));
    }

    #[test]
    fn oversized_panel_is_clamped_and_empty_display_is_ignored() {
        let (position, size) = bottom_centered_bounds(
            PhysicalPosition::new(0, 48),
            PhysicalSize::new(1280, 752),
            PhysicalSize::new(2880, 496),
            12,
        )
        .expect("small display");
        assert_eq!(position, PhysicalPosition::new(12, 292));
        assert_eq!(size, PhysicalSize::new(1256, 496));
        assert!(
            bottom_centered_bounds(
                PhysicalPosition::new(0, 0),
                PhysicalSize::new(0, 0),
                size,
                12
            )
            .is_none()
        );
    }

    #[test]
    fn odd_pixel_remainders_differ_by_at_most_one_and_coordinates_do_not_overflow() {
        let (position, _) = bottom_centered_bounds(
            PhysicalPosition::new(0, -900),
            PhysicalSize::new(1601, 900),
            PhysicalSize::new(1000, 248),
            12,
        )
        .expect("odd width");
        assert_eq!(position, PhysicalPosition::new(300, -260));
        let (position, _) = bottom_centered_bounds(
            PhysicalPosition::new(i32::MAX, i32::MAX),
            PhysicalSize::new(u32::MAX, u32::MAX),
            PhysicalSize::new(1, 1),
            12,
        )
        .expect("bounded arithmetic");
        assert_eq!(position, PhysicalPosition::new(i32::MAX, i32::MAX));
    }
}
