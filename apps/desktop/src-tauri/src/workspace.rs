//! A persistent auxiliary NSPanel. Opening it never changes the rail frame.
use crate::workspace_protocol::{WorkspaceContent, WorkspaceKey, WorkspaceSnapshot};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

#[derive(Default)]
pub struct WorkspaceState(pub Mutex<WorkspaceSnapshot>);

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("workspace")
        .ok_or_else(|| std::io::Error::other("Missing workspace window"))?;
    #[cfg(target_os = "macos")]
    crate::native_panel::install(&window)?;
    crate::window::apply_frame_style(&window).map_err(std::io::Error::other)?;
    crate::window::configure_spaces(&window)?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = dismiss(&handle, true);
        }
    });
    Ok(())
}

fn trace_rail(app: &AppHandle, stage: &'static str) {
    #[cfg(target_os = "macos")]
    if let Some(main) = app.get_webview_window("main")
        && let Ok(pointer) = main.ns_window()
        && let Some(mtm) = objc2::MainThreadMarker::new()
    {
        let native = unsafe { &*(pointer as *const objc2_app_kit::NSWindow) };
        crate::window::trace_invocation(native, mtm, stage);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, stage);
}

pub fn dismiss(app: &AppHandle, focus_main: bool) -> tauri::Result<()> {
    app.state::<WorkspaceState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(WorkspaceContent::Closed);
    if let Some(window) = app.get_webview_window("workspace") {
        window.hide()?;
    }
    trace_rail(app, "workspace-dismissed-rail");
    app.emit_to("workspace", "pasters-workspace", snapshot(app))?;
    app.emit_to("main", "pasters-close-preview", ())?;
    if focus_main
        && let Some(main) = app.get_webview_window("main")
        && main.is_visible()?
    {
        crate::window::present_on_current_space(&main)?;
    }
    Ok(())
}

fn snapshot(app: &AppHandle) -> WorkspaceSnapshot {
    app.state::<WorkspaceState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn position(app: &AppHandle) -> tauri::Result<()> {
    let Some(main) = app.get_webview_window("main") else {
        return Ok(());
    };
    let Some(aux) = app.get_webview_window("workspace") else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWindow;
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let rail = unsafe { &*(main.ns_window()? as *const NSWindow) };
        let panel = unsafe { &*(aux.ns_window()? as *const NSWindow) };
        let Some(screen) = rail.screen() else {
            return Ok(());
        };
        let rail_frame = rail.frame();
        let screen = screen.visibleFrame();
        let width = 760.0_f64.min((screen.size.width - 24.0).max(1.0));
        let top = screen.origin.y + screen.size.height - 12.0;
        let y = rail_frame.origin.y + rail_frame.size.height + 12.0;
        let height = 348.0_f64.min((top - y).max(1.0));
        let x = (rail_frame.origin.x + (rail_frame.size.width - width) / 2.0)
            .clamp(screen.origin.x, screen.origin.x + screen.size.width - width)
            .round();
        let frame = NSRect::new(NSPoint::new(x, y.round()), NSSize::new(width, height));
        if panel.frame() != frame {
            panel.setFrame_display(frame, true);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let scale = main.scale_factor()?;
        let p = main.outer_position()?.to_logical::<f64>(scale);
        let size = main.outer_size()?.to_logical::<f64>(scale);
        aux.set_size(tauri::LogicalSize::new(760.0, 348.0))?;
        aux.set_position(tauri::LogicalPosition::new(
            p.x + (size.width - 760.0) / 2.0,
            p.y - 360.0,
        ))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_workspace(
    app: AppHandle,
    window: WebviewWindow,
    request: WorkspaceContent,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the rail may replace workspace content".into());
    }
    if let WorkspaceContent::Settings { tab } = &request
        && !matches!(
            tab.as_str(),
            "general" | "shortcuts" | "history" | "backup" | "advanced"
        )
    {
        return Err("Unknown settings page".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| -> tauri::Result<()> {
            if matches!(request, WorkspaceContent::Closed) {
                return dismiss(&handle, false);
            }
            handle
                .state::<WorkspaceState>()
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replace(request);
            position(&handle)?;
            handle.emit_to("workspace", "pasters-workspace", snapshot(&handle))
        })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_workspace(app: AppHandle, window: WebviewWindow) -> Result<WorkspaceSnapshot, String> {
    if window.label() != "workspace" {
        return Err("Only the workspace may initialize its content".into());
    }
    Ok(snapshot(&app))
}

#[tauri::command]
pub async fn present_workspace(
    app: AppHandle,
    window: WebviewWindow,
    revision: u64,
) -> Result<(), String> {
    if window.label() != "workspace" {
        return Err("Only the workspace may acknowledge its render".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| -> tauri::Result<()> {
            if !snapshot(&handle).accepts_ready(revision) {
                return Ok(());
            }
            let Some(main) = handle.get_webview_window("main") else {
                return Ok(());
            };
            if !main.is_visible()? {
                return Ok(());
            }
            let Some(aux) = handle.get_webview_window("workspace") else {
                return Ok(());
            };
            position(&handle)?;
            if !aux.is_visible()?
                || matches!(snapshot(&handle).content, WorkspaceContent::Settings { .. })
            {
                crate::window::present_on_current_space(&aux)?;
            }
            trace_rail(&handle, "workspace-presented-rail");
            Ok(())
        })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dismiss_workspace(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != "workspace" {
        return Err("Only the workspace may dismiss itself".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(dismiss(&handle, true).map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_key(
    app: AppHandle,
    window: WebviewWindow,
    request: WorkspaceKey,
) -> Result<(), String> {
    if window.label() != "workspace" || !request.valid() {
        return Err("Invalid workspace navigation".into());
    }
    app.emit_to("main", "pasters-workspace-key", request)
        .map_err(|e| e.to_string())
}
