//! A nonactivating clipboard panel keeps the caller's Space and application.
//! All conversion and panel operations run on the AppKit main thread. The
//! pinned adapter retains Tauri's window delegate and underlying window handle.
use objc2::{ClassType, MainThreadMarker, Message};
use objc2_app_kit::NSWindowStyleMask;
use objc2_foundation::NSObjectProtocol;
use tauri::{Manager, WebviewWindow};
use tauri_nspanel::WebviewWindowExt;

tauri_nspanel::panel!(CopyRailPanel {
    config: {
        can_become_key_window: true,
        can_become_main_window: false,
        becomes_key_only_if_needed: false,
        hides_on_deactivate: false,
    }
});

pub fn install(window: &WebviewWindow) -> tauri::Result<()> {
    MainThreadMarker::new().ok_or_else(|| std::io::Error::other("原生面板必须在主线程创建。"))?;
    let panel = window.to_panel::<CopyRailPanel>()?;
    let style = panel.as_panel().styleMask() | NSWindowStyleMask::NonactivatingPanel;
    panel.set_style_mask(style);
    panel.set_floating_panel(true);
    Ok(())
}
