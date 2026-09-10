#[allow(unsafe_code)]
mod app_icons;
mod build_info;
mod capture_service;
mod cloud_sync;
mod commands;
#[path = "../../src/context_action.rs"]
mod context_action;
#[allow(unsafe_code)]
mod context_menu;
mod drag_export;
#[path = "../../src/drag_feedback.rs"]
mod drag_feedback;
#[allow(unsafe_code)]
mod drag_preview;
#[allow(unsafe_code)]
mod drag_session;
#[allow(unsafe_code)]
mod editor_import;
#[path = "../../src/gesture_trace.rs"]
mod gesture_trace;
#[allow(unsafe_code)]
mod guarded_text_view;
mod locale;
pub mod mcp;
#[allow(unsafe_code)]
#[cfg(target_os = "macos")]
mod native_drop_feedback;
#[allow(unsafe_code)]
mod native_menu;
// Release test binaries must exclude the webview probe too, not just the app.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod native_panel;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod native_space_events;
#[cfg(debug_assertions)]
mod native_tab_diagnostics;
mod native_test;
#[allow(unsafe_code)]
#[cfg(target_os = "macos")]
mod native_window_events;
#[allow(unsafe_code)]
mod pdf_preview;
#[allow(unsafe_code)]
mod rich_text;
#[allow(unsafe_code)]
mod rich_text_editor;
mod shortcut;
#[allow(unsafe_code)]
#[cfg(target_os = "macos")]
mod single_instance;
mod tray_icon;
#[allow(unsafe_code)]
mod window;
#[allow(unsafe_code)]
mod workspace;
#[path = "../../src/workspace_protocol.rs"]
mod workspace_protocol;

use std::{
    fs,
    sync::{Arc, Mutex},
};

use paste_platform::MacPasteTarget;
use paste_storage::SqliteStore;
use tauri::{
    Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::ShortcutState;

use crate::capture_service::CaptureService;

pub struct DesktopState {
    store: Arc<SqliteStore>,
    capture: CaptureService,
    cloud_sync: cloud_sync::CloudSyncService,
    paste_target: MacPasteTarget,
    clipboard_write: tokio::sync::Mutex<()>,
    shortcut: Mutex<shortcut::ShortcutStatus>,
    drag_sessions: Arc<drag_session::DragSessions>,
    native_test: Option<Arc<native_test::NativeTestProfile>>,
}

pub fn run() {
    run_with_profile(None);
}

/// Read compiled configuration without constructing an application, window,
/// plugin, database, or platform clipboard reader.
pub fn build_info_json() -> Result<String, String> {
    let context = application_context();
    serde_json::to_string_pretty(&build_info::BuildInfo::from_config(context.config()))
        .map_err(|error| error.to_string())
}

fn application_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

pub fn run_native_ui_test() -> Result<(), String> {
    let profile = Arc::new(native_test::NativeTestProfile::create()?);
    eprintln!("Native UI test profile: {}", profile.root().display());
    run_with_profile(Some(profile));
    Ok(())
}

pub fn run_native_pdf_test() -> Result<(), String> {
    let profile = Arc::new(native_test::NativeTestProfile::create_pdf()?);
    eprintln!("Native PDF UI test profile: {}", profile.root().display());
    run_with_profile(Some(profile));
    Ok(())
}

pub fn run_native_compact_test(pdf: bool) -> Result<(), String> {
    let profile = Arc::new(native_test::NativeTestProfile::create_compact(pdf)?);
    eprintln!("Native UI test profile: {}", profile.root().display());
    eprintln!("Native UI test layout: compact=true pdf={pdf}");
    run_with_profile(Some(profile));
    Ok(())
}

fn run_with_profile(native_test: Option<Arc<native_test::NativeTestProfile>>) {
    let isolated = native_test.is_some();
    let cleanup = native_test.clone();
    let mut context = application_context();
    if isolated {
        for window in &mut context.config_mut().app.windows {
            window.incognito = true;
        }
    }
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    let builder = if isolated {
        builder
    } else {
        // Must initialize before window creation, hotkeys, databases or capture.
        #[cfg(target_os = "macos")]
        let builder = builder.plugin(single_instance::plugin());
        builder.plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
    };
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            if let Err(error) = window::toggle_main_window(&handle) {
                                eprintln!("CopyRail window invocation failed: {error}");
                            }
                        });
                    }
                })
                .build(),
        )
        .invoke_handler(move |invoke| {
            if isolated
                && matches!(
                    invoke.message.command(),
                    "undo_last_delete"
                        | "start_clip_drag"
                        | "perform_native_text_action"
                        | "hide_window"
                        | "show_window"
                )
            {
                eprintln!("Native UI test command: {}", invoke.message.command());
            }
            if isolated && !native_test::allows_command(invoke.message.command()) {
                eprintln!(
                    "Native UI test denied command: {}",
                    invoke.message.command()
                );
                invoke.resolver.reject(serde_json::json!({
                    "code": "isolated_mode",
                    "message": "隔离验证模式禁止此操作；不会访问系统剪贴板、账户或本机配置。"
                }));
                return true;
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                commands::list_history,
                commands::search_history,
                commands::list_search_facets,
                commands::get_source_icons,
                commands::history_position,
                commands::list_pinboards,
                commands::create_pinboard,
                commands::update_pinboard,
                commands::reorder_pinboards,
                commands::move_pinboard_item,
                commands::place_pinboard_clips,
                commands::delete_pinboard,
                commands::pin_clip,
                commands::pin_clips,
                commands::unpin_clip,
                commands::unpin_clips,
                commands::rename_clip,
                commands::create_textual_item,
                commands::update_textual_item,
                commands::open_rich_text_editor,
                commands::delete_clip,
                commands::delete_clips,
                commands::undo_last_delete,
                commands::perform_native_text_action,
                commands::show_clip_context_menu,
                commands::trace_native_gesture,
                commands::get_clip_preview,
                commands::set_preview_window,
                workspace::update_workspace,
                workspace::get_workspace,
                workspace::present_workspace,
                workspace::dismiss_workspace,
                workspace::workspace_key,
                commands::get_clip_thumbnail,
                commands::rotate_clip_image,
                commands::recognize_clip_text,
                commands::open_link_preview,
                commands::restore_clip,
                commands::restore_clips,
                commands::get_permission_status,
                commands::get_shortcut_status,
                commands::retry_shortcut,
                commands::request_accessibility_permission,
                commands::get_sync_status,
                commands::set_cloud_sync_enabled,
                commands::list_sync_conflicts,
                commands::resolve_sync_conflict,
                commands::list_shared_conflicts,
                commands::resolve_shared_conflict,
                commands::get_mcp_access_status,
                commands::set_mcp_enabled,
                commands::create_mcp_connection,
                commands::revoke_mcp_connection,
                commands::start_clip_drag,
                commands::update_clip_drag_feedback,
                commands::export_backup,
                commands::restore_backup,
                commands::get_capture_preferences,
                commands::update_capture_preferences,
                commands::get_desktop_preferences,
                commands::set_language,
                commands::get_language_settings,
                commands::update_desktop_preferences,
                commands::capture_status,
                commands::pause_capture,
                commands::resume_capture,
                commands::hide_window,
                commands::show_window,
            ];
            handler(invoke)
        })
        .setup(move |app| {
            // This is a tray utility. skipTaskbar does not set the macOS
            // activation policy; a regular app can return to its previous Space.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let shortcut_status = shortcut::register_or_report(app.handle(), isolated);
            let store = if let Some(profile) = &native_test {
                Arc::new(profile.open_seeded_store().map_err(std::io::Error::other)?)
            } else {
                let data_dir = app.path().app_data_dir()?;
                fs::create_dir_all(&data_dir)?;
                Arc::new(SqliteStore::open(data_dir.join("history.db"))?)
            };
            let device = store.get_or_create_device("This Mac")?;
            let preferences = store.load_capture_preferences()?;
            let desktop_preferences = store.load_desktop_preferences()?;
            locale::set_language(desktop_preferences.language);
            let capture = if isolated {
                CaptureService::isolated()
            } else {
                CaptureService::spawn(
                    app.handle().clone(),
                    Arc::clone(&store),
                    device,
                    preferences,
                )
                .map_err(std::io::Error::other)?
            };
            let cloud_sync = if isolated {
                cloud_sync::CloudSyncService::isolated()
            } else {
                cloud_sync::CloudSyncService::spawn(Arc::clone(&store))
            };
            let drag_sessions = Arc::new(drag_session::DragSessions::default());
            if let Some(window) = app.get_webview_window("main") {
                drag_session::install(&window, Arc::clone(&drag_sessions), isolated);
            }
            app.manage(window::PreviewFrame::default());
            app.manage(workspace::WorkspaceState::default());
            app.manage(DesktopState {
                store,
                capture,
                cloud_sync,
                paste_target: MacPasteTarget::default(),
                clipboard_write: tokio::sync::Mutex::new(()),
                shortcut: Mutex::new(shortcut_status),
                drag_sessions,
                native_test: native_test.clone(),
            });
            native_menu::install(app.handle(), isolated)?;
            workspace::install(app.handle())?;

            let show_item =
                MenuItem::with_id(app, "show", locale::t("显示 CopyRail"), true, None::<&str>)?;
            let pause_item = MenuItem::with_id(
                app,
                "pause",
                locale::t("暂停采集 15 分钟"),
                true,
                None::<&str>,
            )?;
            let resume_item =
                MenuItem::with_id(app, "resume", locale::t("继续采集"), true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit_item =
                MenuItem::with_id(app, "quit", locale::t("退出 CopyRail"), true, None::<&str>)?;
            let tray_menu = Menu::with_items(
                app,
                &[
                    &show_item,
                    &pause_item,
                    &resume_item,
                    &separator,
                    &quit_item,
                ],
            )?;
            app.manage(native_menu::TrayLabels {
                show: show_item.clone(),
                pause: pause_item.clone(),
                resume: resume_item.clone(),
                quit: quit_item.clone(),
            });
            TrayIconBuilder::with_id("pasters-tray")
                .icon(tray_icon::template_icon())
                .icon_as_template(true)
                .tooltip("CopyRail")
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window::show_main_window(&window);
                        }
                    }
                    "pause" => {
                        let _ = app.state::<DesktopState>().capture.pause(Some(15));
                    }
                    "resume" => {
                        let _ = app.state::<DesktopState>().capture.resume();
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                native_panel::install(&window)?;
                window::apply_frame_style(&window).map_err(std::io::Error::other)?;
                window::apply_desktop_preferences(&window, desktop_preferences)?;
                window::install_positioning(&window).map_err(std::io::Error::other)?;
                if isolated {
                    window.set_title(
                        app.state::<DesktopState>()
                            .native_test
                            .as_ref()
                            .map_or("CopyRail — 隔离验证", |profile| {
                                profile.window_title()
                            }),
                    )?;
                }
            }
            #[cfg(target_os = "macos")]
            if !isolated {
                single_instance::start_listener(app.handle().clone())?;
            }
            Ok(())
        })
        .build(context)
        .expect("failed to run CopyRail desktop application");
    // setup runs before Tao's applicationDidFinishLaunching in this build/run
    // path. Setting only the live NSApplication policy there is overwritten
    // by Tao's launch policy. Configure the owned runtime before entering run.
    #[cfg(target_os = "macos")]
    let app = {
        let mut app = app;
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        app
    };
    app.run(move |app, event| {
        // Wait for the native launch phase to finish before first ordering.
        // Otherwise Tao's startup activation can reorder our prepared panel.
        if matches!(event, tauri::RunEvent::Ready)
            && let Some(window) = app.get_webview_window("main")
            && let Err(error) = window::show_main_window(&window)
        {
            eprintln!("CopyRail could not show its initial panel: {error}");
        }
        #[cfg(target_os = "macos")]
        if matches!(
            event,
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            }
        ) && let Some(window) = app.get_webview_window("main")
        {
            if isolated {
                eprintln!("Native UI test window: reopen");
            }
            let _ = window::show_main_window(&window);
        }
        if let tauri::RunEvent::ExitRequested { api, .. } = &event
            && !rich_text_editor::confirm_application_exit()
        {
            api.prevent_exit();
        }
        if matches!(event, tauri::RunEvent::Exit)
            && let Some(profile) = &cleanup
        {
            profile.cleanup();
        }
    });
}
