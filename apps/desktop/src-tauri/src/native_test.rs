//! Debug-only, fail-closed native UI validation. This is not a second UI or a
//! fake backend: the real storage, commands, webview and drag session run here.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use paste_domain::{ContentKind, SourceApplication};
use paste_storage::SqliteStore;

pub struct NativeTestProfile {
    #[cfg(debug_assertions)]
    pub tab_diagnostics: crate::native_tab_diagnostics::TabDiagnostics,
    root: PathBuf,
    trace_remaining: AtomicUsize,
    pdf_scenario: bool,
    compact_layout: bool,
}

impl NativeTestProfile {
    pub fn create() -> Result<Self, String> {
        if !cfg!(debug_assertions) {
            return Err("原生隔离验证仅在调试构建中可用。".into());
        }
        // No caller-supplied path can redirect this profile into real history.
        let root = std::env::temp_dir().join(format!("pasters-native-ui-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).map_err(|error| error.to_string())?;
        let profile = Self {
            #[cfg(debug_assertions)]
            tab_diagnostics: crate::native_tab_diagnostics::TabDiagnostics::default(),
            root,
            trace_remaining: AtomicUsize::new(2048),
            pdf_scenario: false,
            compact_layout: false,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&profile.root, fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        fs::create_dir(profile.cache_dir()).map_err(|error| error.to_string())?;
        Ok(profile)
    }

    pub fn create_pdf() -> Result<Self, String> {
        // Reuse the same release rejection, fresh path and permissions.
        let mut profile = Self::create()?;
        profile.pdf_scenario = true;
        Ok(profile)
    }

    pub fn window_title(&self) -> &'static str {
        match (self.pdf_scenario, self.compact_layout) {
            (false, false) => "CopyRail — 隔离验证",
            (true, false) => "CopyRail — PDF 隔离验证",
            (false, true) => "CopyRail — Compact 隔离验证",
            (true, true) => "CopyRail — PDF Compact 隔离验证",
        }
    }

    pub fn create_compact(pdf: bool) -> Result<Self, String> {
        // Both constructors reject release builds before making a directory.
        // Layout is a private fixture preference, not an IPC permission change.
        let mut profile = if pdf {
            Self::create_pdf()?
        } else {
            Self::create()?
        };
        profile.compact_layout = true;
        Ok(profile)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn take_trace_budget(&self, count: usize) -> bool {
        if !(1..=64).contains(&count) {
            return false;
        }
        self.trace_remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(count)
            })
            .is_ok()
    }

    pub fn open_seeded_store(&self) -> Result<SqliteStore, String> {
        let store =
            SqliteStore::open(self.root.join("history.db")).map_err(|error| error.to_string())?;
        let device = store
            .get_or_create_device("Synthetic Mac")
            .map_err(|error| error.to_string())?;
        if self.compact_layout {
            store
                .save_desktop_preferences(paste_domain::DesktopPreferences {
                    compact_mode: true,
                    ..Default::default()
                })
                .map_err(|error| error.to_string())?;
        }
        if self.pdf_scenario {
            self.seed_pdf(&store, device)?;
            return Ok(store);
        }
        let mut boards = Vec::new();
        for name in ["收件箱", "工作", "归档"] {
            boards.push(
                store
                    .create_pinboard(name, "#ff9500")
                    .map_err(|error| error.to_string())?,
            );
        }
        for (index, letter) in ['A', 'B', 'C', 'D', 'E'].iter().enumerate() {
            let board = if index < 4 {
                boards[0].id
            } else {
                boards[1].id
            };
            store
                .create_textual_item_in_pinboard(
                    ContentKind::Text,
                    &format!("合成便签 {letter}\n用于原生拖放验证，不来自真实剪贴板。"),
                    SourceApplication {
                        bundle_identifier: "io.pasters.synthetic".into(),
                        display_name: "Synthetic".into(),
                    },
                    device.clone(),
                    Some(board),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(store)
    }

    #[cfg(debug_assertions)]
    fn seed_pdf(
        &self,
        store: &SqliteStore,
        device: paste_domain::DeviceMetadata,
    ) -> Result<(), String> {
        use paste_domain::{
            CaptureFlags, CapturedItem, CapturedRepresentation, RepresentationKind,
        };
        let board = store
            .create_pinboard("PDF 验收", "#135cc5")
            .map_err(|e| e.to_string())?;
        for (name, bytes) in [
            (
                "native-preview-acceptance.pdf",
                include_bytes!("../fixtures/native-preview-acceptance.pdf").as_slice(),
            ),
            (
                "native-preview-locked.pdf",
                include_bytes!("../fixtures/native-preview-locked.pdf").as_slice(),
            ),
            (
                "native-preview-invalid.pdf",
                b"%PDF-1.4\nSynthetic invalid document, not user data.\n".as_slice(),
            ),
        ] {
            let item = store
                .insert_capture(&CapturedItem {
                    captured_at: chrono::Utc::now(),
                    source: SourceApplication {
                        bundle_identifier: "io.pasters.synthetic".into(),
                        display_name: "Synthetic PDF".into(),
                    },
                    device: device.clone(),
                    flags: CaptureFlags::default(),
                    representations: vec![CapturedRepresentation {
                        kind: RepresentationKind::Pdf,
                        native_type: Some("com.adobe.pdf".into()),
                        mime_type: Some("application/pdf".into()),
                        file_name: Some(name.into()),
                        bytes: bytes.to_vec(),
                    }],
                })
                .map_err(|e| e.to_string())?;
            store
                .pin_clip(board.id, item.id)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[cfg(not(debug_assertions))]
    fn seed_pdf(
        &self,
        _store: &SqliteStore,
        _device: paste_domain::DeviceMetadata,
    ) -> Result<(), String> {
        Err("原生隔离验证仅在调试构建中可用。".into())
    }

    pub fn cleanup(&self) {
        // The private field is always the single fresh UUID directory above.
        // Removing a symlink at this path does not follow it into its target.
        if let Err(error) = fs::remove_dir_all(&self.root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("Native UI test cleanup failed: {error}");
        }
    }
}

impl Drop for NativeTestProfile {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Unknown future commands are denied until explicitly reviewed for isolation.
pub fn allows_command(command: &str) -> bool {
    matches!(
        command,
        "list_history"
            | "search_history"
            | "list_search_facets"
            | "history_position"
            | "list_pinboards"
            | "create_pinboard"
            | "update_pinboard"
            | "reorder_pinboards"
            | "move_pinboard_item"
            | "place_pinboard_clips"
            | "delete_pinboard"
            | "pin_clip"
            | "pin_clips"
            | "unpin_clip"
            | "unpin_clips"
            | "rename_clip"
            | "create_textual_item"
            | "update_textual_item"
            | "delete_clip"
            | "delete_clips"
            | "undo_last_delete"
            | "perform_native_text_action" // Command separately rejects Cut / Copy / Paste.
            | "show_clip_context_menu" // Read-only snapshot and native choice, no direct action.
            | "trace_native_gesture" // Typed metadata, isolated-only and bounded per profile.
            | "get_clip_preview"
            | "update_workspace"
            | "get_workspace"
            | "present_workspace"
            | "dismiss_workspace"
            | "workspace_key"
            | "set_preview_window" // Main-window geometry only; no external data or clipboard.
            | "get_clip_thumbnail"
            | "get_source_icons" // Existing synthetic clip IDs only; installed icon metadata, no launch.
            | "rotate_clip_image"
            | "recognize_clip_text"
            | "get_permission_status"
            | "get_shortcut_status" // Cached status only; no system registration.
            | "get_sync_status"
            | "list_sync_conflicts"
            | "list_shared_conflicts"
            | "get_mcp_access_status"
            | "get_capture_preferences"
            | "get_desktop_preferences"
            | "set_language"
            | "get_language_settings" // Read system locale and the isolated preference only.
            | "capture_status"
            | "start_clip_drag"
            | "update_clip_drag_feedback" // Geometry only, active native session and main-window scoped.
            | "show_window"
            | "hide_window"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(debug_assertions)]
    fn diagnostics_are_bounded_per_batch_and_profile() {
        let profile = NativeTestProfile::create().expect("isolated profile");
        assert!(!profile.take_trace_budget(0));
        assert!(!profile.take_trace_budget(65));
        for _ in 0..32 {
            assert!(profile.take_trace_budget(64));
        }
        assert!(!profile.take_trace_budget(1));
    }

    #[test]
    fn unknown_and_system_side_effect_commands_fail_closed() {
        for command in [
            "open_rich_text_editor", // Native text controls expose system clipboard actions.
            "restore_clip",
            "restore_clips",
            "resume_capture",
            "pause_capture",
            "request_accessibility_permission",
            "retry_shortcut",
            "set_cloud_sync_enabled",
            "resolve_sync_conflict",
            "resolve_shared_conflict",
            "create_mcp_connection",
            "set_mcp_enabled",
            "revoke_mcp_connection",
            "export_backup",
            "restore_backup",
            "update_desktop_preferences",
            "update_capture_preferences",
            "open_link_preview",
            "a_future_command",
            "plugin:autostart|enable",
        ] {
            assert!(!allows_command(command), "must deny {command}");
        }
        for command in [
            "list_history",
            "place_pinboard_clips",
            "start_clip_drag",
            "update_clip_drag_feedback",
            "get_source_icons",
            "get_shortcut_status",
            "reorder_pinboards",
        ] {
            assert!(allows_command(command), "must exercise the real {command}");
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn fresh_profiles_have_only_synthetic_content_and_remove_only_their_own_directory() {
        let first = NativeTestProfile::create().expect("fresh debug profile");
        let second = NativeTestProfile::create().expect("second independent profile");
        assert_ne!(first.root(), second.root());
        let store = first.open_seeded_store().expect("real synthetic database");
        let boards = store.list_pinboards().expect("boards");
        assert_eq!(
            boards
                .iter()
                .map(|board| board.item_count)
                .collect::<Vec<_>>(),
            vec![4, 1, 0]
        );
        assert!(!store.cloud_sync_enabled().expect("cloud disabled"));
        let path = first.root().to_owned();
        drop(store);
        drop(first);
        assert!(!path.exists());
        assert!(second.root().exists());
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn release_build_rejects_native_ui_test_before_creating_any_profile() {
        assert!(NativeTestProfile::create().is_err());
        assert!(NativeTestProfile::create_pdf().is_err());
        assert!(NativeTestProfile::create_compact(false).is_err());
        assert!(NativeTestProfile::create_compact(true).is_err());
    }

    #[test]
    #[cfg(debug_assertions)]
    fn compact_fixtures_change_only_the_private_layout_preference() {
        for pdf in [false, true] {
            let profile = NativeTestProfile::create_compact(pdf).expect("compact fixture");
            let store = profile.open_seeded_store().expect("synthetic store");
            assert_eq!(
                store.load_desktop_preferences().expect("preferences"),
                paste_domain::DesktopPreferences {
                    compact_mode: true,
                    ..Default::default()
                }
            );
            assert_eq!(
                profile.window_title(),
                if pdf {
                    "CopyRail — PDF Compact 隔离验证"
                } else {
                    "CopyRail — Compact 隔离验证"
                }
            );
            assert_eq!(
                store
                    .list_pinboards()
                    .expect("boards")
                    .iter()
                    .map(|b| b.item_count)
                    .collect::<Vec<_>>(),
                if pdf { vec![3] } else { vec![4, 1, 0] }
            );
            assert!(!store.cloud_sync_enabled().expect("cloud disabled"));
            assert!(!allows_command("update_desktop_preferences"));
            assert!(!allows_command("update_capture_preferences"));
            assert!(!allows_command("restore_clip"));
            let other = NativeTestProfile::create().expect("independent default");
            assert_ne!(profile.root(), other.root());
            assert!(
                !other
                    .open_seeded_store()
                    .expect("default store")
                    .load_desktop_preferences()
                    .expect("default layout")
                    .compact_mode
            );
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn pdf_scenario_is_fresh_synthetic_and_does_not_change_drag_fixture_or_privacy() {
        let profile = NativeTestProfile::create_pdf().expect("PDF profile");
        let store = profile
            .open_seeded_store()
            .expect("real synthetic PDF storage");
        assert_eq!(profile.window_title(), "CopyRail — PDF 隔离验证");
        let boards = store.list_pinboards().expect("PDF board");
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].item_count, 3);
        let items = store
            .search_items(
                &paste_domain::SearchQuery::default(),
                paste_domain::SearchPage::default(),
            )
            .expect("PDF history");
        assert_eq!(items.len(), 3);
        assert!(
            items
                .iter()
                .all(|item| item.content_kind == ContentKind::Pdf
                    && item.source.bundle_identifier == "io.pasters.synthetic")
        );
        for item in &items {
            let payload = store.load_clipboard_payload(item.id).expect("PDF original");
            assert_eq!(payload.len(), 1);
            let expected_pages = if item.title.contains("acceptance") {
                Some(3)
            } else {
                None
            };
            assert_eq!(
                crate::pdf_preview::inspect(&payload[0].bytes).ok(),
                expected_pages
            );
        }
        assert!(!store.cloud_sync_enabled().expect("cloud remains disabled"));
        assert!(!allows_command("restore_clip"));
        assert!(!allows_command("open_rich_text_editor"));
        let drag = NativeTestProfile::create().expect("independent default profile");
        assert_ne!(profile.root(), drag.root());
        assert_eq!(
            drag.open_seeded_store()
                .expect("default fixture")
                .list_pinboards()
                .expect("default boards")
                .iter()
                .map(|board| board.item_count)
                .collect::<Vec<_>>(),
            vec![4, 1, 0]
        );
    }
}
