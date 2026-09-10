//! Route menu accelerators through the same context as the timeline keyboard.
//! AppKit text windows keep the responder chain; the main webview decides
//! between editing text and operating on clipboard-history items.
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl EditAction {
    fn from_menu_id(id: &str) -> Option<Self> {
        Some(match id {
            "pasters-edit-undo" => Self::Undo,
            "pasters-edit-redo" => Self::Redo,
            "pasters-edit-cut" => Self::Cut,
            "pasters-edit-copy" => Self::Copy,
            "pasters-edit-paste" => Self::Paste,
            "pasters-edit-select-all" => Self::SelectAll,
            _ => return None,
        })
    }

    pub fn permitted_in_isolation(self) -> bool {
        matches!(self, Self::Undo | Self::Redo | Self::SelectAll)
    }
}

pub fn install(app: &AppHandle, isolated: bool) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        let Some(submenu) = item.as_submenu() else {
            continue;
        };
        if submenu.text()? == "Edit" {
            for item in submenu.items()? {
                submenu.remove(&item)?;
            }
            for (id, title, accelerator) in [
                ("pasters-edit-undo", "撤销", "CmdOrCtrl+Z"),
                ("pasters-edit-redo", "重做", "CmdOrCtrl+Shift+Z"),
                ("pasters-edit-cut", "剪切", "CmdOrCtrl+X"),
                ("pasters-edit-copy", "复制", "CmdOrCtrl+C"),
                ("pasters-edit-paste", "粘贴", "CmdOrCtrl+V"),
                ("pasters-edit-select-all", "全选", "CmdOrCtrl+A"),
            ] {
                if id == "pasters-edit-cut" {
                    submenu.append(&PredefinedMenuItem::separator(app)?)?;
                }
                submenu.append(&MenuItem::with_id(
                    app,
                    id,
                    crate::locale::t(title),
                    true,
                    Some(accelerator),
                )?)?;
            }
        }
        if submenu.text()? == "Window" {
            submenu.prepend(&MenuItem::with_id(
                app,
                "pasters-show-main",
                crate::locale::t("显示 CopyRail"),
                true,
                // Normal mode already owns the global shortcut. Registering
                // it twice would toggle twice; QA only gets an in-app item.
                isolated.then_some("CmdOrCtrl+Shift+V"),
            )?)?;
        }
    }
    app.set_menu(menu)?;
    app.on_menu_event(move |app, event| {
        if event.id().as_ref() == "pasters-show-main" {
            if isolated {
                eprintln!("Native UI test window: menu show");
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = crate::window::show_main_window(&window);
            }
            return;
        }
        let Some(action) = EditAction::from_menu_id(event.id().as_ref()) else {
            return;
        };
        if let Some(main) = ["main", "workspace"]
            .into_iter()
            .filter_map(|label| app.get_webview_window(label))
            .find(|window| window.is_focused().unwrap_or(false))
        {
            if isolated {
                eprintln!("Native UI test menu: {action:?}");
            }
            let _ = main.emit("pasters-edit-action", action);
        } else if !isolated {
            // E.g. the actual AppKit rich-text editor or a link-preview window.
            let _ = send_to_responder(action);
        }
    });
    Ok(())
}

pub struct TrayLabels {
    pub show: MenuItem<tauri::Wry>,
    pub pause: MenuItem<tauri::Wry>,
    pub resume: MenuItem<tauri::Wry>,
    pub quit: MenuItem<tauri::Wry>,
}

/// Change existing items; never register event handlers again on locale changes.
pub fn relabel(app: &AppHandle) -> tauri::Result<()> {
    if let Some(menu) = app.menu() {
        for entry in menu.items()? {
            if let Some(submenu) = entry.as_submenu() {
                for entry in submenu.items()? {
                    if let Some(item) = entry.as_menuitem() {
                        let key = match item.id().as_ref() {
                            "pasters-edit-undo" => "撤销",
                            "pasters-edit-redo" => "重做",
                            "pasters-edit-cut" => "剪切",
                            "pasters-edit-copy" => "复制",
                            "pasters-edit-paste" => "粘贴",
                            "pasters-edit-select-all" => "全选",
                            "pasters-show-main" => "显示 CopyRail",
                            _ => continue,
                        };
                        item.set_text(crate::locale::t(key))?;
                    }
                }
            }
        }
    }
    if let Some(tray) = app.try_state::<TrayLabels>() {
        for (item, key) in [
            (&tray.show, "显示 CopyRail"),
            (&tray.pause, "暂停采集 15 分钟"),
            (&tray.resume, "继续采集"),
            (&tray.quit, "退出 CopyRail"),
        ] {
            item.set_text(crate::locale::t(key))?;
        }
    }
    Ok(())
}

pub fn send_to_responder(action: EditAction) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        use objc2::sel;
        use objc2_app_kit::NSApplication;
        use objc2_foundation::MainThreadMarker;
        let mtm = MainThreadMarker::new().ok_or("文本操作需要主线程。")?;
        let selector = match action {
            EditAction::Undo => sel!(undo:),
            EditAction::Redo => sel!(redo:),
            EditAction::Cut => sel!(cut:),
            EditAction::Copy => sel!(copy:),
            EditAction::Paste => sel!(paste:),
            EditAction::SelectAll => sel!(selectAll:),
        };
        // Only fixed, standard AppKit selectors; no JS or caller-supplied selector.
        Ok(unsafe {
            NSApplication::sharedApplication(mtm).sendAction_to_from(selector, None, None)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = action;
        Err("当前平台尚不支持原生文本菜单。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_actions_are_exact_and_unknown_actions_fail_closed() {
        for (id, wire, action) in [
            ("undo", "undo", EditAction::Undo),
            ("redo", "redo", EditAction::Redo),
            ("cut", "cut", EditAction::Cut),
            ("copy", "copy", EditAction::Copy),
            ("paste", "paste", EditAction::Paste),
            ("select-all", "select_all", EditAction::SelectAll),
        ] {
            assert_eq!(
                EditAction::from_menu_id(&format!("pasters-edit-{id}")),
                Some(action)
            );
            assert_eq!(serde_json::to_value(action).expect("action"), wire);
            assert_eq!(
                serde_json::from_value::<EditAction>(serde_json::json!(wire)).expect("wire"),
                action
            );
        }
        for id in ["undo", "pasters-edit-Undo", "pasters-edit-delete", ""] {
            assert!(EditAction::from_menu_id(id).is_none());
        }
        assert!(serde_json::from_str::<EditAction>("\"delete\"").is_err());
    }

    #[test]
    fn isolated_native_text_actions_never_touch_the_system_clipboard() {
        for action in [EditAction::Undo, EditAction::Redo, EditAction::SelectAll] {
            assert!(action.permitted_in_isolation());
        }
        for action in [EditAction::Cut, EditAction::Copy, EditAction::Paste] {
            assert!(!action.permitted_in_isolation());
        }
    }
}
