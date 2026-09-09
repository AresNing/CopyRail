use std::{collections::HashSet, path::Path, str::FromStr, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{TimeZone, Utc};
use paste_domain::{
    CapturePreferences, ClipId, ClipItem, ContentKind, DesktopPreferences, DeviceId, Pinboard,
    PinboardId, RepresentationKind, SearchFacets, SearchFilters, SearchPage, SearchQuery,
    SourceApplication,
};
use paste_storage::{CachedPreview, SharedConflictResolution, SyncConflictResolution};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::{
    DesktopState,
    capture_service::CaptureStatus,
    window::{apply_desktop_preferences, hide_main_window, show_main_window},
};

type ApiResult<T> = Result<T, ApiError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIconRequest {
    clip_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIconResult {
    bundle_identifier: String,
    data_url: Option<String>,
}

#[tauri::command]
pub async fn get_source_icons(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: SourceIconRequest,
) -> ApiResult<Vec<SourceIconResult>> {
    if window.label() != "main" {
        return Err(ApiError::invalid("来源图标只允许主窗口有界读取。"));
    }
    let bundles = source_icon_keys(&state.store, request.clip_ids)?;
    let mut result = Vec::new();
    for bundle_identifier in bundles {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let key = bundle_identifier.clone();
        // One icon per run-loop job so a batch cannot monopolize the main thread.
        app.run_on_main_thread(move || {
            let _ = sender.send(crate::app_icons::load(&key).unwrap_or(None));
        })
        .map_err(ApiError::storage)?;
        let png = receiver.await.map_err(ApiError::storage)?;
        result.push(SourceIconResult {
            bundle_identifier,
            data_url: png.map(|png| format!("data:image/png;base64,{}", STANDARD.encode(png))),
        });
    }
    Ok(result)
}

fn source_icon_keys(
    store: &paste_storage::SqliteStore,
    values: Vec<String>,
) -> ApiResult<Vec<String>> {
    if values.len() > crate::app_icons::policy::MAX_ICON_BATCH {
        return Err(ApiError::invalid("一次最多读取 16 个来源图标。"));
    }
    let ids = parse_clip_ids(values)?;
    // Resolve the entire selection before native IO. The caller cannot pass
    // arbitrary application identifiers or paths to probe installed apps.
    let mut seen = HashSet::new();
    let mut bundles = Vec::new();
    for id in ids {
        let clip = store
            .get_clip(id)
            .map_err(ApiError::storage)?
            .ok_or_else(|| ApiError::invalid("来源条目已不存在。"))?;
        if seen.insert(clip.source.bundle_identifier.clone()) {
            bundles.push(clip.source.bundle_identifier);
        }
    }
    Ok(bundles)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureTraceRequest {
    events: Vec<crate::gesture_trace::GestureTrace>,
}

#[tauri::command]
pub fn trace_native_gesture(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: GestureTraceRequest,
) -> ApiResult<()> {
    // Normal application mode cannot log user interaction through this command.
    let profile = state
        .native_test
        .as_ref()
        .filter(|_| window.label() == "main")
        .ok_or_else(|| ApiError::invalid("手势诊断只允许独立隔离窗口。"))?;
    if !profile.take_trace_budget(request.events.len()) {
        return Ok(());
    }
    for event in request.events {
        eprintln!(
            "Native UI test gesture: {}",
            serde_json::to_string(&event).map_err(|error| ApiError::invalid(error.to_string()))?
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn perform_native_text_action(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    action: crate::native_menu::EditAction,
) -> ApiResult<bool> {
    if window.label() != "main" || (state.native_test.is_some() && !action.permitted_in_isolation())
    {
        return Err(ApiError::invalid("隔离验证模式禁止系统剪贴板操作。"));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = if window.is_focused().unwrap_or(false) {
            crate::native_menu::send_to_responder(action)
        } else {
            Err("文本焦点已离开此窗口，请重新操作。".into())
        };
        let _ = sender.send(result);
    })
    .map_err(ApiError::storage)?;
    receiver
        .await
        .map_err(ApiError::storage)?
        .map_err(ApiError::invalid)
}

#[tauri::command]
pub async fn open_rich_text_editor(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: PreviewRequest,
) -> ApiResult<()> {
    let id = ClipId::from_str(&request.clip_id).map_err(|_| ApiError::invalid("无效内容 ID。"))?;
    let store = std::sync::Arc::clone(&state.store);
    let snapshot = store
        .rich_text_edit_snapshot(id)
        .map_err(ApiError::storage)?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let editor_app = app.clone();
    app.run_on_main_thread(move || {
        let _ = sender.send(crate::rich_text_editor::open(&editor_app, store, snapshot));
    })
    .map_err(ApiError::storage)?;
    receiver
        .await
        .map_err(ApiError::storage)?
        .map_err(ApiError::invalid)
}

#[derive(Deserialize)]
pub struct ClipContextMenuRequest {
    clip_ids: Vec<String>,
    x: f64,
    y: f64,
}

#[derive(Serialize)]
pub struct ClipContextMenuChoice {
    choice: crate::context_action::ContextAction,
    item: Option<ClipItem>,
}

#[tauri::command]
pub async fn show_clip_context_menu(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: ClipContextMenuRequest,
) -> ApiResult<Option<ClipContextMenuChoice>> {
    if window.label() != "main" {
        return Err(ApiError::invalid(
            "Only the main window may open a clip menu",
        ));
    }
    let ids = parse_clip_ids(request.clip_ids)?;
    let context = state
        .store
        .clip_action_context(&ids)
        .map_err(ApiError::storage)?;
    let isolated = state.native_test.is_some();
    let language = state
        .store
        .load_desktop_preferences()
        .map_err(ApiError::storage)?
        .language;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result =
            crate::context_menu::popup(&window, &context, isolated, language, request.x, request.y)
                .map(|choice| {
                    choice.map(|choice| {
                        // Use the chosen item's database snapshot, not later UI state.
                        let item = if matches!(
                            choice,
                            crate::context_action::ContextAction::Edit
                                | crate::context_action::ContextAction::Rename
                        ) {
                            context.clips.into_iter().next().map(|item| item.clip)
                        } else {
                            None
                        };
                        ClipContextMenuChoice { choice, item }
                    })
                });
        if isolated {
            match &result {
                Ok(choice) => eprintln!(
                    "Native UI test context menu: {:?}",
                    choice.as_ref().map(|choice| choice.choice)
                ),
                Err(error) => eprintln!("Native UI test context menu error: {error}"),
            }
        }
        let _ = sender.send(result);
    })
    .map_err(ApiError::storage)?;
    receiver
        .await
        .map_err(ApiError::storage)?
        .map_err(ApiError::invalid)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    code: &'static str,
    message: String,
}

impl ApiError {
    fn storage(error: impl std::fmt::Display) -> Self {
        Self {
            code: "storage_error",
            message: error.to_string(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request",
            message: message.into(),
        }
    }

    fn preview(error: impl std::fmt::Display) -> Self {
        Self {
            code: "preview_error",
            message: error.to_string(),
        }
    }

    fn ocr(error: impl std::fmt::Display) -> Self {
        Self {
            code: "ocr_error",
            message: error.to_string(),
        }
    }

    fn link_preview(error: impl std::fmt::Display) -> Self {
        Self {
            code: "link_preview_error",
            message: error.to_string(),
        }
    }

    fn drag(error: impl std::fmt::Display) -> Self {
        Self {
            code: "drag_error",
            message: error.to_string(),
        }
    }

    fn permission(message: impl Into<String>) -> Self {
        Self {
            code: "permission_required",
            message: message.into(),
        }
    }

    fn mcp(error: impl std::fmt::Display) -> Self {
        Self {
            code: "mcp_error",
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct HistoryRequest {
    limit: u32,
    offset: u32,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    text: String,
    pinboard_id: Option<String>,
    content_kinds: Vec<paste_domain::ContentKind>,
    source_bundle_ids: Vec<String>,
    device_ids: Vec<String>,
    copied_after_ms: Option<i64>,
    copied_before_ms: Option<i64>,
    limit: u32,
    offset: u32,
}

#[derive(Debug, Deserialize)]
pub struct PauseRequest {
    minutes: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PinboardRequest {
    name: String,
    color: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePinboardRequest {
    pinboard_id: String,
    name: String,
    color: String,
}

#[derive(Debug, Deserialize)]
pub struct ReorderPinboardsRequest {
    pinboard_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MovePinboardItemRequest {
    pinboard_id: String,
    clip_id: String,
    direction: i8,
}

#[derive(Deserialize)]
pub struct PlacePinboardClipsRequest {
    pinboard_id: String,
    clip_ids: Vec<String>,
    anchor: Option<String>,
    after: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeletePinboardRequest {
    pinboard_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PinClipRequest {
    pinboard_id: String,
    clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PinClipsRequest {
    pinboard_id: String,
    clip_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    clip_id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTextualItemRequest {
    kind: ContentKind,
    value: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTextualItemRequest {
    clip_id: String,
    kind: ContentKind,
    title: String,
    value: String,
}

#[derive(Debug, Deserialize)]
pub struct ClipRequest {
    clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ClipsRequest {
    clip_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RotateImageRequest {
    clip_id: String,
    direction: i8,
}

#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    clip_id: String,
    plain_text: bool,
    paste: bool,
}

#[derive(Debug, Deserialize)]
pub struct RestoreManyRequest {
    clip_ids: Vec<String>,
    plain_text: bool,
    paste: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    clipboard_change_count: i64,
    paste_requested: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    clip_id: String,
    media_type: String,
    data_url: String,
    pixel_width: u32,
    pixel_height: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    item: ClipItem,
    line_count: usize,
    character_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragExportResult {
    item_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartDragRequest {
    clip_ids: Vec<String>,
    #[serde(default)]
    feedback: Option<crate::drag_feedback::DragLayout>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupActionResult {
    path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPositionResult {
    position: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    accessibility_trusted: bool,
    app_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    enabled: bool,
    local_outbox_ready: bool,
    cloud_transport_configured: bool,
    syncing: bool,
    last_success_at_ms: Option<i64>,
    pending_changes: u64,
    pending_conflicts: u64,
    pending_dependencies: usize,
    pending_shared_downloads: usize,
    pending_shared_conflicts: usize,
    blocked_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CloudSyncEnabledRequest {
    enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflictView {
    id: String,
    local_title: String,
    remote_title: String,
    remote_would_win: bool,
    created_at_ms: i64,
}

#[derive(Debug, Deserialize)]
pub struct ResolveSyncConflictRequest {
    conflict_id: String,
    resolution: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConflictVersionView {
    title: String,
    preview: String,
    device_name: String,
    deleted: bool,
    timestamp_ms: i64,
}

impl From<paste_storage::SharedConflictVersion> for SharedConflictVersionView {
    fn from(value: paste_storage::SharedConflictVersion) -> Self {
        Self {
            title: value.title,
            preview: value.preview,
            device_name: value.device_name,
            deleted: value.deleted,
            timestamp_ms: value.timestamp_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConflictView {
    id: String,
    pinboard_name: String,
    can_resolve: bool,
    current_operation_id: String,
    current: SharedConflictVersionView,
    first: SharedConflictVersionView,
    second: SharedConflictVersionView,
}

#[derive(Debug, Deserialize)]
pub struct ResolveSharedConflictRequest {
    conflict_id: String,
    current_operation_id: String,
    resolution: String,
}

#[derive(Debug, Deserialize)]
pub struct McpEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateMcpConnectionRequest {
    display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct RevokeMcpConnectionRequest {
    client_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientView {
    id: String,
    display_name: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAccessStatus {
    enabled: bool,
    clients: Vec<McpClientView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionResult {
    status: McpAccessStatus,
    configuration: String,
}

#[tauri::command]
pub fn get_permission_status() -> PermissionStatus {
    PermissionStatus {
        accessibility_trusted: paste_platform::MacPasteTarget::accessibility_permission_granted(),
        app_path: current_bundle_path(),
    }
}

fn bundle_path_for_executable(executable: &Path) -> Option<String> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_string_lossy().into_owned())
}

fn current_bundle_path() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|path| bundle_path_for_executable(&path))
}

#[tauri::command]
pub fn get_shortcut_status(
    state: State<'_, DesktopState>,
) -> ApiResult<crate::shortcut::ShortcutStatus> {
    state
        .shortcut
        .lock()
        .map(|status| status.clone())
        .map_err(|_| ApiError::invalid("快捷键状态暂时不可读。"))
}

#[tauri::command]
pub async fn retry_shortcut(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> ApiResult<crate::shortcut::ShortcutStatus> {
    if window.label() != "main" || state.native_test.is_some() {
        return Err(ApiError::invalid(
            "只能在正常主窗口中重新启用快捷键；隔离验证不会注册系统快捷键。",
        ));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if sender.is_closed() {
            return;
        }
        // Serialize native registration on AppKit, without holding a state lock
        // on an async worker while it waits for the main thread.
        let status = crate::shortcut::register_or_report(&handle, false);
        let state = handle.state::<DesktopState>();
        let result = state
            .shortcut
            .lock()
            .map(|mut current| {
                *current = status.clone();
                status
            })
            .map_err(|_| ApiError::invalid("快捷键状态暂时不可写。"));
        let _ = sender.send(result);
    })
    .map_err(ApiError::storage)?;
    receiver.await.map_err(ApiError::storage)?
}

#[tauri::command]
pub fn request_accessibility_permission() -> PermissionStatus {
    PermissionStatus {
        accessibility_trusted: paste_platform::MacPasteTarget::request_accessibility_permission(),
        app_path: current_bundle_path(),
    }
}

#[tauri::command]
pub fn get_sync_status(state: State<'_, DesktopState>) -> ApiResult<SyncStatus> {
    let enabled = state
        .store
        .cloud_sync_enabled()
        .map_err(ApiError::storage)?;
    let runtime = state.cloud_sync.status();
    let pending_changes = state
        .store
        .pending_sync_change_count()
        .map_err(ApiError::storage)?;
    let pending_dependencies = state
        .store
        .pending_remote_membership_count()
        .map_err(ApiError::storage)?;
    let pending_shared_downloads = state
        .store
        .pending_shared_download_count()
        .map_err(ApiError::storage)?;
    let pending_shared_conflicts = state
        .store
        .shared_conflict_count()
        .map_err(ApiError::storage)?;
    state
        .store
        .pending_sync_conflict_count()
        .map(|pending_conflicts| SyncStatus {
            enabled,
            local_outbox_ready: true,
            cloud_transport_configured: runtime.transport_configured,
            syncing: runtime.syncing,
            last_success_at_ms: runtime.last_success_at_ms,
            pending_changes,
            pending_conflicts,
            pending_dependencies,
            pending_shared_downloads,
            pending_shared_conflicts,
            blocked_reason: enabled.then_some(runtime.blocked_reason).flatten(),
        })
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn set_cloud_sync_enabled(
    state: State<'_, DesktopState>,
    request: CloudSyncEnabledRequest,
) -> ApiResult<SyncStatus> {
    state
        .store
        .set_cloud_sync_enabled(request.enabled)
        .map_err(ApiError::storage)?;
    state.cloud_sync.wake();
    get_sync_status(state)
}

#[tauri::command]
pub fn list_sync_conflicts(state: State<'_, DesktopState>) -> ApiResult<Vec<SyncConflictView>> {
    load_sync_conflicts(&state)
}

#[tauri::command]
pub fn resolve_sync_conflict(
    state: State<'_, DesktopState>,
    request: ResolveSyncConflictRequest,
) -> ApiResult<Vec<SyncConflictView>> {
    let conflict_id = uuid::Uuid::parse_str(&request.conflict_id)
        .map_err(|_| ApiError::invalid("conflict_id must be a UUID"))?;
    let resolution = parse_sync_conflict_resolution(&request.resolution)?;
    state
        .store
        .resolve_sync_conflict(conflict_id, resolution)
        .map_err(ApiError::storage)?;
    state.cloud_sync.wake();
    load_sync_conflicts(&state)
}

fn parse_sync_conflict_resolution(value: &str) -> ApiResult<SyncConflictResolution> {
    Ok(match value {
        "keep_local" => SyncConflictResolution::KeepLocal,
        "accept_remote" => SyncConflictResolution::AcceptRemote,
        _ => {
            return Err(ApiError::invalid(
                "resolution must be keep_local or accept_remote",
            ));
        }
    })
}

fn load_sync_conflicts(state: &DesktopState) -> ApiResult<Vec<SyncConflictView>> {
    state
        .store
        .list_sync_conflicts(100)
        .map(|conflicts| {
            conflicts
                .into_iter()
                .map(|conflict| SyncConflictView {
                    id: conflict.id.to_string(),
                    local_title: conflict.local_title,
                    remote_title: conflict.remote_title,
                    remote_would_win: conflict.remote_would_win,
                    created_at_ms: conflict.created_at.timestamp_millis(),
                })
                .collect()
        })
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn list_shared_conflicts(state: State<'_, DesktopState>) -> ApiResult<Vec<SharedConflictView>> {
    load_shared_conflicts(&state)
}

fn load_shared_conflicts(state: &DesktopState) -> ApiResult<Vec<SharedConflictView>> {
    state
        .store
        .list_shared_conflicts(50)
        .map(|items| {
            items
                .into_iter()
                .map(|item| SharedConflictView {
                    id: item.id.to_string(),
                    pinboard_name: item.pinboard_name,
                    can_resolve: item.can_resolve,
                    current_operation_id: item.current_operation_id.to_string(),
                    current: item.current.into(),
                    first: item.first.into(),
                    second: item.second.into(),
                })
                .collect()
        })
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn resolve_shared_conflict(
    state: State<'_, DesktopState>,
    request: ResolveSharedConflictRequest,
) -> ApiResult<Vec<SharedConflictView>> {
    let id = request.conflict_id.parse().map_err(ApiError::storage)?;
    let current = uuid::Uuid::parse_str(&request.current_operation_id)
        .map_err(|_| ApiError::invalid("共享版本 ID 无效。"))?;
    let resolution = parse_shared_conflict_resolution(&request.resolution)?;
    state
        .store
        .resolve_shared_conflict(id, current, resolution)
        .map_err(|error| match error {
            paste_storage::StorageError::ReadOnlyPinboardShare => ApiError {
                code: "share_read_only",
                message: "当前共享板为只读，未应用选择。".into(),
            },
            other => ApiError::storage(other),
        })?;
    state.cloud_sync.wake();
    load_shared_conflicts(&state)
}

fn parse_shared_conflict_resolution(value: &str) -> ApiResult<SharedConflictResolution> {
    match value {
        "keep_current" => Ok(SharedConflictResolution::KeepCurrent),
        "use_first" => Ok(SharedConflictResolution::UseFirst),
        "use_second" => Ok(SharedConflictResolution::UseSecond),
        _ => Err(ApiError::invalid(
            "请选择保留当前内容、采用版本 A 或采用版本 B。",
        )),
    }
}

#[tauri::command]
pub fn get_mcp_access_status(state: State<'_, DesktopState>) -> ApiResult<McpAccessStatus> {
    load_mcp_access_status(&state)
}

#[tauri::command]
pub fn set_mcp_enabled(
    state: State<'_, DesktopState>,
    request: McpEnabledRequest,
) -> ApiResult<McpAccessStatus> {
    state
        .store
        .set_mcp_enabled(request.enabled)
        .map_err(ApiError::storage)?;
    load_mcp_access_status(&state)
}

#[tauri::command]
pub fn create_mcp_connection(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: CreateMcpConnectionRequest,
) -> ApiResult<McpConnectionResult> {
    let executable = std::env::current_exe().map_err(ApiError::mcp)?;
    let database = app
        .path()
        .app_data_dir()
        .map_err(ApiError::mcp)?
        .join("history.db");
    let token = format!(
        "pasters_mcp_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let configuration = mcp_connection_configuration(&executable, &database, &token)?;
    let token_hash = *blake3::hash(token.as_bytes()).as_bytes();
    state
        .store
        .register_mcp_client(&request.display_name, &token_hash)
        .map_err(ApiError::storage)?;
    state
        .store
        .set_mcp_enabled(true)
        .map_err(ApiError::storage)?;
    Ok(McpConnectionResult {
        status: load_mcp_access_status(&state)?,
        configuration,
    })
}

#[tauri::command]
pub fn revoke_mcp_connection(
    state: State<'_, DesktopState>,
    request: RevokeMcpConnectionRequest,
) -> ApiResult<McpAccessStatus> {
    let client_id = uuid::Uuid::parse_str(&request.client_id)
        .map_err(|_| ApiError::invalid("invalid MCP client id"))?;
    state
        .store
        .revoke_mcp_client(client_id)
        .map_err(ApiError::storage)?;
    load_mcp_access_status(&state)
}

fn load_mcp_access_status(state: &State<'_, DesktopState>) -> ApiResult<McpAccessStatus> {
    let enabled = state.store.mcp_enabled().map_err(ApiError::storage)?;
    let clients = state
        .store
        .list_mcp_clients()
        .map_err(ApiError::storage)?
        .into_iter()
        .map(|client| McpClientView {
            id: client.id.to_string(),
            display_name: client.display_name,
            created_at: client.created_at.to_rfc3339(),
            last_used_at: client.last_used_at.map(|value| value.to_rfc3339()),
        })
        .collect();
    Ok(McpAccessStatus { enabled, clients })
}

fn mcp_connection_configuration(
    executable: &Path,
    database: &Path,
    token: &str,
) -> ApiResult<String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| ApiError::invalid("application path is not valid UTF-8"))?;
    let database = database
        .to_str()
        .ok_or_else(|| ApiError::invalid("database path is not valid UTF-8"))?;
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "pasters": {
                "command": executable,
                "args": ["--mcp-stdio"],
                "env": {
                    "PASTERS_DATABASE_PATH": database,
                    "PASTERS_MCP_TOKEN": token
                }
            }
        }
    }))
    .map_err(ApiError::mcp)
}

#[tauri::command]
pub fn list_history(
    _window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: HistoryRequest,
) -> ApiResult<Vec<ClipItem>> {
    #[cfg(debug_assertions)]
    if let Some(profile) = &state.native_test {
        profile.tab_diagnostics.observe(&_window, None, false);
    }
    state
        .store
        .list_history(SearchPage::new(request.limit, request.offset))
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn search_history(
    _window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: SearchRequest,
) -> ApiResult<Vec<ClipItem>> {
    let pinboard_ids: Vec<PinboardId> = request
        .pinboard_id
        .map(|value| {
            PinboardId::from_str(&value).map_err(|error| ApiError::invalid(error.to_string()))
        })
        .transpose()?
        .into_iter()
        .collect();
    #[cfg(debug_assertions)]
    if let Some(profile) = &state.native_test {
        let search = !request.text.is_empty()
            || !request.content_kinds.is_empty()
            || !request.source_bundle_ids.is_empty()
            || !request.device_ids.is_empty()
            || request.copied_after_ms.is_some()
            || request.copied_before_ms.is_some();
        profile
            .tab_diagnostics
            .observe(&_window, pinboard_ids.first().copied(), search);
    }
    let device_ids = request
        .device_ids
        .into_iter()
        .map(|value| {
            DeviceId::from_str(&value).map_err(|error| ApiError::invalid(error.to_string()))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let copied_after = request.copied_after_ms.map(parse_timestamp).transpose()?;
    let copied_before = request.copied_before_ms.map(parse_timestamp).transpose()?;
    state
        .store
        .search_items(
            &SearchQuery {
                text: request.text,
                filters: SearchFilters {
                    pinboard_ids,
                    content_kinds: request.content_kinds,
                    source_bundle_ids: request.source_bundle_ids,
                    device_ids,
                    copied_after,
                    copied_before,
                },
            },
            SearchPage::new(request.limit, request.offset),
        )
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn list_search_facets(state: State<'_, DesktopState>) -> ApiResult<SearchFacets> {
    state.store.list_search_facets().map_err(ApiError::storage)
}

#[tauri::command]
pub fn history_position(
    state: State<'_, DesktopState>,
    request: ClipRequest,
) -> ApiResult<HistoryPositionResult> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .history_position(clip_id)
        .map(|position| HistoryPositionResult { position })
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn list_pinboards(state: State<'_, DesktopState>) -> ApiResult<Vec<Pinboard>> {
    state.store.list_pinboards().map_err(ApiError::storage)
}

#[tauri::command]
pub fn create_pinboard(
    state: State<'_, DesktopState>,
    request: PinboardRequest,
) -> ApiResult<Pinboard> {
    state
        .store
        .create_pinboard(&request.name, &request.color)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn update_pinboard(
    state: State<'_, DesktopState>,
    request: UpdatePinboardRequest,
) -> ApiResult<Pinboard> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .update_pinboard(pinboard_id, &request.name, &request.color)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn reorder_pinboards(
    state: State<'_, DesktopState>,
    request: ReorderPinboardsRequest,
) -> ApiResult<Vec<Pinboard>> {
    let pinboard_ids = request
        .pinboard_ids
        .into_iter()
        .map(|value| {
            PinboardId::from_str(&value).map_err(|error| ApiError::invalid(error.to_string()))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    state
        .store
        .reorder_pinboards(&pinboard_ids)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn move_pinboard_item(
    state: State<'_, DesktopState>,
    request: MovePinboardItemRequest,
) -> ApiResult<bool> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .move_pinboard_item(pinboard_id, clip_id, request.direction)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn place_pinboard_clips(
    state: State<'_, DesktopState>,
    request: PlacePinboardClipsRequest,
) -> ApiResult<bool> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    let anchor = request
        .anchor
        .map(|value| ClipId::from_str(&value))
        .transpose()
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .place_pinboard_clips(pinboard_id, &clip_ids, anchor, request.after)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub async fn delete_pinboard(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: DeletePinboardRequest,
) -> ApiResult<bool> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let pinboard = state
        .store
        .list_pinboards()
        .map_err(ApiError::storage)?
        .into_iter()
        .find(|item| item.id == pinboard_id)
        .ok_or_else(|| ApiError::invalid("pinboard does not exist"))?;
    let confirmed = window
        .dialog()
        .message(format!(
            "将删除 Pinboard“{}”及其归属关系。剪贴板历史不会被删除。",
            pinboard.name
        ))
        .title("删除这个 Pinboard？")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "删除 Pinboard".into(),
            "取消".into(),
        ))
        .blocking_show();
    if !confirmed {
        return Ok(false);
    }
    state
        .store
        .delete_pinboard(pinboard_id)
        .map_err(ApiError::storage)?;
    Ok(true)
}

#[tauri::command]
pub fn pin_clip(state: State<'_, DesktopState>, request: PinClipRequest) -> ApiResult<()> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .pin_clip(pinboard_id, clip_id)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn pin_clips(state: State<'_, DesktopState>, request: PinClipsRequest) -> ApiResult<()> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    state
        .store
        .pin_clips(pinboard_id, &clip_ids)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn unpin_clip(state: State<'_, DesktopState>, request: PinClipRequest) -> ApiResult<()> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .unpin_clip(pinboard_id, clip_id)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn unpin_clips(state: State<'_, DesktopState>, request: PinClipsRequest) -> ApiResult<()> {
    let pinboard_id = PinboardId::from_str(&request.pinboard_id)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    state
        .store
        .unpin_clips(pinboard_id, &clip_ids)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn rename_clip(state: State<'_, DesktopState>, request: RenameRequest) -> ApiResult<()> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .rename_clip(clip_id, &request.title)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn create_textual_item(
    state: State<'_, DesktopState>,
    request: CreateTextualItemRequest,
) -> ApiResult<ClipItem> {
    let device = state
        .store
        .get_or_create_device("This Mac")
        .map_err(ApiError::storage)?;
    state
        .store
        .create_textual_item(
            request.kind,
            &request.value,
            SourceApplication {
                bundle_identifier: "io.pasters.desktop".into(),
                display_name: "CopyRail".into(),
            },
            device,
        )
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn update_textual_item(
    state: State<'_, DesktopState>,
    request: UpdateTextualItemRequest,
) -> ApiResult<ClipItem> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .update_textual_clip(clip_id, request.kind, &request.title, &request.value)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn delete_clip(state: State<'_, DesktopState>, request: ClipRequest) -> ApiResult<()> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    state
        .store
        .delete_clip(clip_id, Utc::now())
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn delete_clips(state: State<'_, DesktopState>, request: ClipsRequest) -> ApiResult<()> {
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    state
        .store
        .delete_clips(&clip_ids, Utc::now())
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn undo_last_delete(state: State<'_, DesktopState>) -> ApiResult<Vec<ClipItem>> {
    state
        .store
        .undo_last_delete_batch()
        .map_err(ApiError::storage)
}

#[tauri::command]
pub async fn get_clip_thumbnail(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: PreviewRequest,
) -> ApiResult<PreviewResult> {
    if window.label() != "main" {
        return Err(ApiError::invalid("缩略图仅允许主窗口读取。"));
    }
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    // Raster work is not performed in WebKit's/main AppKit event loop. One
    // decoder at a time bounds concurrent native PDF/image resource usage.
    static RASTER_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let permit = RASTER_GATE.acquire().await.map_err(ApiError::preview)?;
    let store = std::sync::Arc::clone(&state.store);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        load_thumbnail(&store, clip_id).map(|preview| preview_result(clip_id, preview))
    })
    .await
    .map_err(ApiError::preview)?
}

fn load_thumbnail(store: &paste_storage::SqliteStore, clip_id: ClipId) -> ApiResult<CachedPreview> {
    let item = store
        .get_clip(clip_id)
        .map_err(ApiError::storage)?
        .ok_or_else(|| ApiError::invalid("preview item does not exist"))?;
    if !matches!(item.content_kind, ContentKind::Image | ContentKind::Pdf) {
        return Err(ApiError::preview(
            "this content type has no raster thumbnail",
        ));
    }
    if let Some(preview) = store
        .load_cached_preview(clip_id, &item.content_hash)
        .map_err(ApiError::storage)?
    {
        return Ok(preview);
    }
    let representations = store
        .load_clipboard_payload(clip_id)
        .map_err(ApiError::storage)?;
    let generated = if item.content_kind == ContentKind::Pdf {
        let pdf = representations
            .iter()
            .find(|value| value.kind == RepresentationKind::Pdf)
            .ok_or_else(|| ApiError::preview("PDF representation is unavailable"))?;
        crate::pdf_preview::render(&pdf.bytes, 1_200, 900).map_err(ApiError::preview)?
    } else {
        paste_platform::generate_image_thumbnail(&representations, 1_200, 900)
            .map_err(ApiError::preview)?
    };
    if store
        .get_clip(clip_id)
        .map_err(ApiError::storage)?
        .is_none_or(|current| current.content_hash != item.content_hash)
    {
        return Err(ApiError::preview("内容已变化，请重新生成缩略图。"));
    }
    let preview = CachedPreview {
        media_type: generated.media_type,
        bytes: generated.bytes,
        pixel_width: generated.pixel_width,
        pixel_height: generated.pixel_height,
    };
    store
        .save_cached_preview(clip_id, &item.content_hash, &preview)
        .map_err(ApiError::storage)?;
    Ok(preview)
}

#[tauri::command]
pub async fn get_clip_preview(
    state: State<'_, DesktopState>,
    request: PreviewRequest,
) -> ApiResult<PreviewResult> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    let item = state
        .store
        .get_clip(clip_id)
        .map_err(ApiError::storage)?
        .ok_or_else(|| ApiError::invalid("preview item does not exist"))?;

    match item.content_kind {
        ContentKind::Image => {
            let preview = match state
                .store
                .load_cached_preview(clip_id, &item.content_hash)
                .map_err(ApiError::storage)?
            {
                Some(preview) => preview,
                None => {
                    let representations = state
                        .store
                        .load_clipboard_payload(clip_id)
                        .map_err(ApiError::storage)?;
                    let generated =
                        paste_platform::generate_image_thumbnail(&representations, 1_200, 900)
                            .map_err(ApiError::preview)?;
                    let preview = CachedPreview {
                        media_type: generated.media_type,
                        bytes: generated.bytes,
                        pixel_width: generated.pixel_width,
                        pixel_height: generated.pixel_height,
                    };
                    state
                        .store
                        .save_cached_preview(clip_id, &item.content_hash, &preview)
                        .map_err(ApiError::storage)?;
                    preview
                }
            };
            Ok(preview_result(clip_id, preview))
        }
        ContentKind::Pdf => {
            // CoreGraphics parsing and base64 allocation must not run on the
            // window event loop or stall async command dispatch.
            let store = std::sync::Arc::clone(&state.store);
            tokio::task::spawn_blocking(move || load_pdf_preview(&store, clip_id))
                .await
                .map_err(ApiError::preview)?
        }
        _ => Err(ApiError::preview(
            "this content type uses the built-in text preview",
        )),
    }
}

fn load_pdf_preview(
    store: &paste_storage::SqliteStore,
    clip_id: ClipId,
) -> ApiResult<PreviewResult> {
    let representations = store
        .load_clipboard_payload(clip_id)
        .map_err(ApiError::storage)?;
    let bytes = representations
        .iter()
        .find(|value| value.kind == RepresentationKind::Pdf)
        .map(|value| value.bytes.as_slice())
        .ok_or_else(|| ApiError::preview("PDF representation is unavailable"))?;
    crate::pdf_preview::inspect(bytes).map_err(ApiError::preview)?;
    Ok(PreviewResult {
        clip_id: clip_id.to_string(),
        media_type: "application/pdf".into(),
        data_url: data_url("application/pdf", bytes),
        pixel_width: 0,
        pixel_height: 0,
    })
}

#[tauri::command]
pub fn rotate_clip_image(
    state: State<'_, DesktopState>,
    request: RotateImageRequest,
) -> ApiResult<ClipItem> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    let direction = match request.direction {
        -1 => paste_platform::ImageRotation::CounterClockwise,
        1 => paste_platform::ImageRotation::Clockwise,
        _ => {
            return Err(ApiError::invalid(
                "image rotation direction must be -1 or 1",
            ));
        }
    };
    let representations = state
        .store
        .load_clipboard_payload(clip_id)
        .map_err(ApiError::storage)?;
    let rotated =
        paste_platform::rotate_image(&representations, direction).map_err(ApiError::preview)?;
    state
        .store
        .update_image_content(clip_id, &rotated.bytes)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub async fn recognize_clip_text(
    state: State<'_, DesktopState>,
    request: ClipRequest,
) -> ApiResult<OcrResult> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    let representations = state
        .store
        .load_clipboard_payload(clip_id)
        .map_err(ApiError::storage)?;
    let bytes = representations
        .into_iter()
        .find(|value| {
            matches!(
                value.kind,
                RepresentationKind::Png | RepresentationKind::Tiff
            )
        })
        .map(|value| value.bytes)
        .ok_or_else(|| ApiError::ocr("image representation is unavailable"))?;
    let recognized =
        tauri::async_runtime::spawn_blocking(move || paste_platform::recognize_text(&bytes))
            .await
            .map_err(ApiError::ocr)?
            .map_err(ApiError::ocr)?;
    let line_count = recognized.line_count;
    let character_count = recognized.text.chars().count();
    let item = state
        .store
        .update_ocr_text(clip_id, &recognized.text)
        .map_err(ApiError::storage)?;
    Ok(OcrResult {
        item,
        line_count,
        character_count,
    })
}

#[tauri::command]
pub fn open_link_preview(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: ClipRequest,
) -> ApiResult<()> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    let item = state
        .store
        .get_clip(clip_id)
        .map_err(ApiError::storage)?
        .ok_or_else(|| ApiError::invalid("link item does not exist"))?;
    if item.content_kind != ContentKind::Link {
        return Err(ApiError::invalid("selected item is not a link"));
    }
    let representations = state
        .store
        .load_clipboard_payload(clip_id)
        .map_err(ApiError::storage)?;
    let raw_url = representations
        .iter()
        .find(|value| value.kind == RepresentationKind::Url)
        .and_then(paste_domain::CapturedRepresentation::utf8_text)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(item.searchable_text.trim());
    let url = parse_link_preview_url(raw_url)?;

    let label = format!("link-preview-{clip_id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.navigate(url).map_err(ApiError::link_preview)?;
        window.show().map_err(ApiError::link_preview)?;
        window.set_focus().map_err(ApiError::link_preview)?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(url))
        .title("CopyRail · 链接预览")
        .inner_size(1_080.0, 760.0)
        .min_inner_size(640.0, 420.0)
        .center()
        .on_navigation(|url| matches!(url.scheme(), "http" | "https"))
        .build()
        .map_err(ApiError::link_preview)?;
    Ok(())
}

fn parse_link_preview_url(raw_url: &str) -> ApiResult<url::Url> {
    let url = url::Url::parse(raw_url).map_err(|_| ApiError::link_preview("invalid link URL"))?;
    if matches!(url.scheme(), "http" | "https") {
        Ok(url)
    } else {
        Err(ApiError::link_preview(
            "only HTTP and HTTPS links can be previewed",
        ))
    }
}

fn preview_result(clip_id: ClipId, preview: CachedPreview) -> PreviewResult {
    PreviewResult {
        clip_id: clip_id.to_string(),
        media_type: preview.media_type.clone(),
        data_url: data_url(&preview.media_type, &preview.bytes),
        pixel_width: preview.pixel_width,
        pixel_height: preview.pixel_height,
    }
}

fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", STANDARD.encode(bytes))
}

#[tauri::command]
pub async fn restore_clip(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: RestoreRequest,
) -> ApiResult<RestoreResult> {
    let clip_id =
        ClipId::from_str(&request.clip_id).map_err(|error| ApiError::invalid(error.to_string()))?;
    let mode = if request.plain_text {
        paste_platform::ClipboardWriteMode::PlainText
    } else {
        paste_platform::ClipboardWriteMode::Original
    };
    restore_items(window, state, vec![clip_id], mode, request.paste).await
}

#[tauri::command]
pub async fn restore_clips(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: RestoreManyRequest,
) -> ApiResult<RestoreResult> {
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    let mode = if request.plain_text {
        paste_platform::ClipboardWriteMode::PlainText
    } else {
        paste_platform::ClipboardWriteMode::Original
    };
    restore_items(window, state, clip_ids, mode, request.paste).await
}

#[tauri::command]
pub async fn update_clip_drag_feedback(
    app: AppHandle,
    window: WebviewWindow,
    request: crate::drag_feedback::LayoutUpdate,
) -> ApiResult<bool> {
    if window.label() != "main" {
        return Err(ApiError::invalid("仅主窗口可以更新拖动提示。"));
    }
    let id = uuid::Uuid::parse_str(&request.session_id)
        .map_err(|_| ApiError::invalid("拖动会话已失效。"))?;
    if request.revision == 0 || !request.layout.valid() {
        return Err(ApiError::invalid("拖动目标布局已失效。"));
    }
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let accepted =
                crate::drag_session::update_native_feedback(id, request.revision, request.layout);
            let _ = sender.send(accepted);
        })
        .map_err(ApiError::drag)?;
        receiver.await.map_err(ApiError::drag)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id);
        Ok(false)
    }
}

#[tauri::command]
pub async fn start_clip_drag(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: StartDragRequest,
) -> ApiResult<DragExportResult> {
    if window.label() != "main" {
        return Err(ApiError::invalid("仅主窗口可以启动内容拖动。"));
    }
    let diagnostics = state.native_test.is_some();
    let started = std::time::Instant::now();
    let clip_ids = parse_clip_ids(request.clip_ids)?;
    let feedback = match request.feedback {
        Some(layout) if layout.valid() => Some(crate::drag_session::Feedback::from_context(
            layout,
            state
                .store
                .clip_action_context(&clip_ids)
                .map_err(ApiError::storage)?,
        )),
        Some(_) => return Err(ApiError::invalid("拖动目标布局已失效，请重试。")),
        None => None,
    };
    let first_clip = state
        .store
        .get_clip(clip_ids[0])
        .map_err(ApiError::storage)?
        .ok_or_else(|| ApiError::invalid("拖动内容已不存在。"))?;
    let mut preview = crate::drag_preview::DragPreview::new(&first_clip, clip_ids.len());
    if matches!(
        first_clip.content_kind,
        ContentKind::Image | ContentKind::Pdf
    ) && let Some(cached) = state
        .store
        .load_cached_preview(first_clip.id, &first_clip.content_hash)
        .map_err(ApiError::storage)?
        && cached.media_type == "image/png"
    {
        // Missing/unusable visual cache falls back to metadata, never replaces
        // or alters the separately validated original drag export.
        let _ = preview.set_thumbnail(cached.bytes);
    }
    let cache_dir = if let Some(profile) = &state.native_test {
        profile.cache_dir()
    } else {
        app.path().app_cache_dir().map_err(ApiError::drag)?
    };
    let paths = crate::drag_export::prepare_drag_files(&state.store, &cache_dir, &clip_ids)
        .map_err(ApiError::drag)?;
    let item_count = paths.len();
    let sessions = std::sync::Arc::clone(&state.drag_sessions);
    let session_id = sessions
        .start(paths.clone(), clip_ids, std::time::Instant::now())
        .map_err(ApiError::drag)?;
    #[cfg(target_os = "macos")]
    let timeout_app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        if diagnostics {
            eprintln!(
                "Native UI test drag begin: elapsed_ms={} files={item_count}",
                started.elapsed().as_millis()
            );
            #[cfg(target_os = "macos")]
            eprintln!(
                "Native UI test drag buttons: {}",
                objc2_app_kit::NSEvent::pressedMouseButtons()
            );
        }
        #[cfg(target_os = "macos")]
        if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 == 0 {
            // The WebKit IPC may arrive after a cancelled/finished gesture.
            // Never start a phantom AppKit session after the button was released.
            sessions.cancel(session_id);
            let _ = sender.send(Err("鼠标已松开，拖动已取消；请按住卡片再拖动。".to_owned()));
            return;
        }
        let completion = std::sync::Arc::clone(&sessions);
        let destination = window.clone();
        if let Some(icon) = crate::app_icons::cached(&first_clip.source.bundle_identifier) {
            let _ = preview.set_source_icon(icon);
        }
        let image = match crate::drag_preview::render(
            &preview,
            window.theme().ok() == Some(tauri::Theme::Dark),
        ) {
            Ok(image) => image,
            Err(error) => {
                sessions.cancel(session_id);
                let _ = sender.send(Err(error));
                return;
            }
        };
        if diagnostics {
            // Only the dedicated synthetic profile writes this QA artifact.
            // Normal drags do not persist a preview of clipboard contents.
            if std::fs::write(cache_dir.join("last-drag-preview.tiff"), &image).is_err() {
                eprintln!("Native UI test preview artifact could not be saved");
            }
        }
        #[cfg(target_os = "macos")]
        if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 == 0 {
            sessions.cancel(session_id);
            let _ = sender.send(Err("鼠标已松开，拖动已取消。".to_owned()));
            return;
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = crate::drag_session::begin_native_destination(
            &window,
            std::sync::Arc::clone(&sessions),
            session_id,
            diagnostics,
            feedback,
            &image,
        ) {
            sessions.cancel(session_id);
            let _ = sender.send(Err(error));
            return;
        }
        #[cfg(target_os = "macos")]
        let completed = std::sync::Arc::new(tokio::sync::Notify::new());
        #[cfg(target_os = "macos")]
        let completion_signal = std::sync::Arc::clone(&completed);
        let result = drag::start_drag(
            &window,
            drag::DragItem::Files(paths),
            drag::Image::Raw(image),
            move |result, _cursor_position| {
                if diagnostics {
                    eprintln!("Native UI test drag ended: {result:?}");
                }
                let cancelled = matches!(result, drag::DragResult::Cancel);
                if cancelled {
                    completion.cancel(session_id);
                } else {
                    completion.finish(session_id, std::time::Instant::now());
                }
                #[cfg(target_os = "macos")]
                {
                    crate::drag_session::end_native_destination(session_id);
                    completion_signal.notify_one();
                }
                // The browser may not receive dragend after a native session.
                let _ = tauri::Emitter::emit(
                    &destination,
                    "pasters-drag-ended",
                    crate::drag_session::DragEndedEvent {
                        session_id,
                        cancelled,
                    },
                );
            },
            drag::Options {
                mode: drag::DragMode::Copy,
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string());
        if diagnostics {
            eprintln!("Native UI test drag begin result: {result:?}");
        }
        if result.is_err() {
            #[cfg(target_os = "macos")]
            crate::drag_session::end_native_destination(session_id);
            sessions.cancel(session_id);
        }
        #[cfg(target_os = "macos")]
        if result.is_ok() {
            // Never leave an invisible input-catching view behind if a native
            // source callback is lost. Match the existing 120s session bound.
            tauri::async_runtime::spawn(async move {
                if tokio::time::timeout(Duration::from_secs(120), completed.notified())
                    .await
                    .is_err()
                {
                    let _ = timeout_app.run_on_main_thread(move || {
                        if crate::drag_session::end_native_destination(session_id) {
                            sessions.cancel(session_id);
                            let _ = tauri::Emitter::emit(
                                &window,
                                "pasters-drag-ended",
                                crate::drag_session::DragEndedEvent {
                                    session_id,
                                    cancelled: true,
                                },
                            );
                        }
                    });
                }
            });
        }
        let _ = sender.send(result);
    })
    .map_err(ApiError::drag)?;
    receiver
        .recv()
        .map_err(|error| ApiError::drag(error.to_string()))?
        .map_err(ApiError::drag)?;
    Ok(DragExportResult { item_count })
}

#[tauri::command]
pub async fn export_backup(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> ApiResult<Option<BackupActionResult>> {
    let file_name = format!("CopyRail-{}.pasters-backup", Utc::now().format("%Y-%m-%d"));
    let selected = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(crate::locale::t("导出 CopyRail 本地备份"))
        .set_file_name(file_name)
        .add_filter("CopyRail Backup", &["pasters-backup"])
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(ApiError::storage)?;
    state
        .store
        .export_backup(&path)
        .map_err(ApiError::storage)?;
    Ok(Some(BackupActionResult {
        path: path.display().to_string(),
    }))
}

#[tauri::command]
pub async fn restore_backup(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> ApiResult<Option<BackupActionResult>> {
    let selected = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(crate::locale::t("选择 CopyRail 本地备份"))
        .add_filter("CopyRail Backup", &["pasters-backup"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(ApiError::storage)?;
    let confirmed = window
        .dialog()
        .message(crate::locale::t(
            "恢复会用备份中的历史、Pinboards 和本地设置替换当前数据。此操作完成后不能自动撤销。",
        ))
        .title(crate::locale::t("恢复 CopyRail 备份？"))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            crate::locale::t("恢复备份").into(),
            crate::locale::t("取消").into(),
        ))
        .blocking_show();
    if !confirmed {
        return Ok(None);
    }

    state
        .store
        .restore_backup(&path)
        .map_err(ApiError::storage)?;
    let capture_preferences = state
        .store
        .load_capture_preferences()
        .map_err(ApiError::storage)?;
    state
        .capture
        .update_preferences(capture_preferences)
        .map_err(ApiError::storage)?;
    let desktop_preferences = state
        .store
        .load_desktop_preferences()
        .map_err(ApiError::storage)?;
    apply_desktop_preferences(&window, desktop_preferences).map_err(ApiError::storage)?;
    crate::locale::set_language(desktop_preferences.language);
    let _ = crate::native_menu::relabel(window.app_handle());
    Ok(Some(BackupActionResult {
        path: path.display().to_string(),
    }))
}

async fn restore_items(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    clip_ids: Vec<ClipId>,
    mode: paste_platform::ClipboardWriteMode,
    paste: bool,
) -> ApiResult<RestoreResult> {
    if window.label() != "main" || state.native_test.is_some() {
        return Err(ApiError::permission(
            "仅正常模式的主窗口可以写回系统剪贴板。",
        ));
    }
    // Refuse overlap instead of letting a second copy overwrite a pending
    // paste. Dropping the command/guard does not queue a late, surprise paste.
    let _write_guard = state
        .clipboard_write
        .try_lock()
        .map_err(|_| ApiError::invalid("上一项复制或粘贴仍在处理中，请稍后再试。"))?;
    let invocation = on_restore_main_thread(&window, |window| {
        ensure_restore_focus(window.is_focused().map_err(ApiError::storage)?)?;
        window
            .state::<DesktopState>()
            .paste_target
            .snapshot()
            .map_err(ApiError::storage)
    })
    .await?;
    // Storage reads run in the async command worker, not the AppKit loop.
    let representations = clip_ids
        .into_iter()
        .map(|id| state.store.load_clipboard_payload(id))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::storage)?;
    let write_invocation = invocation.clone();
    let (clipboard_change_count, attempt) = on_restore_main_thread(&window, move |window| {
        let state = window.state::<DesktopState>();
        ensure_restore_focus(window.is_focused().map_err(ApiError::storage)?)?;
        if !state
            .paste_target
            .is_current(&write_invocation)
            .map_err(ApiError::storage)?
        {
            return Err(ApiError::invalid(
                "窗口已重新打开，本次操作已取消；剪贴板未改动。",
            ));
        }
        let count = paste_platform::MacClipboardWriter::write_many(&representations, mode)
            .map_err(ApiError::storage)?;
        if !paste {
            return Ok((count, None));
        }
        ensure_direct_paste_permission(
            paste_platform::MacPasteTarget::accessibility_permission_granted(),
        )?;
        if !write_invocation.has_target() {
            return Ok((count, None));
        }
        // This is an internal handoff, not a user dismissal: retain this one
        // invocation. All public hide paths invalidate pending attempts.
        window.hide().map_err(ApiError::storage)?;
        Ok((count, Some(write_invocation.begin(count))))
    })
    .await?;
    let paste_requested = if let Some(attempt) = attempt {
        let result = run_paste_attempt(&window, attempt)
            .await
            .and_then(paste_outcome_result);
        if !matches!(result, Ok(true)) {
            // Do not steal focus back after a user switch or a new invocation.
            let _ = on_restore_main_thread(&window, move |window| {
                if window
                    .state::<DesktopState>()
                    .paste_target
                    .is_current(&invocation)
                    .map_err(ApiError::storage)?
                    && paste_platform::MacPasteTarget::owns_foreground()
                {
                    show_main_window(&window).map_err(ApiError::storage)?;
                }
                Ok(())
            })
            .await;
        }
        result?
    } else {
        false
    };
    Ok(RestoreResult {
        clipboard_change_count,
        paste_requested,
    })
}

async fn on_restore_main_thread<T: Send + 'static>(
    window: &WebviewWindow,
    action: impl FnOnce(WebviewWindow) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let main = window.clone();
    window
        .app_handle()
        .run_on_main_thread(move || {
            if !sender.is_closed() {
                let _ = sender.send(action(main));
            }
        })
        .map_err(ApiError::storage)?;
    receiver.await.map_err(ApiError::storage)?
}

async fn run_paste_attempt(
    window: &WebviewWindow,
    mut attempt: paste_platform::MacPasteAttempt,
) -> ApiResult<paste_platform::PasteOutcome> {
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    loop {
        // Yield between observations so AppKit can update activation state.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (next, progress) = on_restore_main_thread(window, move |window| {
            let progress = window
                .state::<DesktopState>()
                .paste_target
                .advance(&mut attempt, deadline)
                .map_err(|_| ApiError::invalid("自动粘贴未完成，请重新选择内容或手动粘贴。"))?;
            Ok((attempt, progress))
        })
        .await?;
        attempt = next;
        if let paste_platform::PasteProgress::Complete(outcome) = progress {
            return Ok(outcome);
        }
    }
}

fn ensure_restore_focus(focused: bool) -> ApiResult<()> {
    if focused {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "CopyRail 已失去焦点，本次操作已取消；剪贴板未改动。",
        ))
    }
}

fn paste_outcome_result(outcome: paste_platform::PasteOutcome) -> ApiResult<bool> {
    use paste_platform::PasteOutcome;
    match outcome {
        PasteOutcome::Requested => Ok(true),
        PasteOutcome::NoTarget => Ok(false),
        PasteOutcome::PermissionDenied => ensure_direct_paste_permission(false).map(|()| false),
        PasteOutcome::ClipboardChanged => Err(ApiError::invalid(
            "系统剪贴板已变化，已取消自动粘贴；请重新选择内容。",
        )),
        PasteOutcome::SessionChanged | PasteOutcome::FocusChanged => Err(ApiError::invalid(
            "窗口或目标已切换，已取消自动粘贴；请确认内容后手动粘贴。",
        )),
        PasteOutcome::TargetUnavailable => Err(ApiError::invalid(
            "内容已复制；原目标应用已退出或身份已变化，请手动粘贴。",
        )),
        PasteOutcome::ActivationRefused | PasteOutcome::ActivationTimedOut => Err(
            ApiError::invalid("内容已复制；目标应用未获得焦点，请切换到目标应用手动粘贴。"),
        ),
        PasteOutcome::PostFailed => Err(ApiError::invalid(
            "自动粘贴未完成，请重新选择内容或手动粘贴。",
        )),
    }
}

fn ensure_direct_paste_permission(accessibility_trusted: bool) -> ApiResult<()> {
    if accessibility_trusted {
        Ok(())
    } else {
        Err(ApiError::permission(
            "内容已复制；自动粘贴需要在 macOS“系统设置 → 隐私与安全性 → 辅助功能”中允许 CopyRail。",
        ))
    }
}

fn parse_clip_ids(values: Vec<String>) -> ApiResult<Vec<ClipId>> {
    if values.is_empty() {
        return Err(ApiError::invalid("at least one clip id is required"));
    }
    if values.len() > 200 {
        return Err(ApiError::invalid("a batch can contain at most 200 items"));
    }
    let mut seen = HashSet::with_capacity(values.len());
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let clip_id =
            ClipId::from_str(&value).map_err(|error| ApiError::invalid(error.to_string()))?;
        if seen.insert(clip_id) {
            result.push(clip_id);
        }
    }
    Ok(result)
}

fn parse_timestamp(milliseconds: i64) -> ApiResult<chrono::DateTime<Utc>> {
    Utc.timestamp_millis_opt(milliseconds)
        .single()
        .ok_or_else(|| ApiError::invalid("timestamp is outside the supported range"))
}

#[tauri::command]
pub fn get_capture_preferences(state: State<'_, DesktopState>) -> ApiResult<CapturePreferences> {
    state
        .store
        .load_capture_preferences()
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn update_capture_preferences(
    state: State<'_, DesktopState>,
    request: CapturePreferences,
) -> ApiResult<CapturePreferences> {
    let saved = state
        .store
        .save_capture_preferences(request)
        .map_err(ApiError::storage)?;
    state
        .capture
        .update_preferences(saved.clone())
        .map_err(ApiError::storage)?;
    Ok(saved)
}

/// Language changes never touch login, capture, Accessibility or window geometry.
#[tauri::command]
pub fn set_language(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: paste_domain::Language,
) -> ApiResult<paste_domain::Language> {
    let saved = state
        .store
        .save_language(request)
        .map_err(ApiError::storage)?;
    crate::locale::set_language(saved);
    if let Err(error) = crate::native_menu::relabel(&app) {
        eprintln!("Could not refresh native menu language: {error}");
    }
    Ok(saved)
}

#[tauri::command]
pub fn get_desktop_preferences(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> ApiResult<DesktopPreferences> {
    read_desktop_preferences(&state.store, state.native_test.is_some(), || {
        app.autolaunch().is_enabled().map_err(ApiError::storage)
    })
}

fn read_desktop_preferences(
    store: &paste_storage::SqliteStore,
    isolated: bool,
    read_system_login: impl FnOnce() -> ApiResult<bool>,
) -> ApiResult<DesktopPreferences> {
    let mut preferences = store
        .load_desktop_preferences()
        .map_err(ApiError::storage)?;
    // The isolated app intentionally has no autostart plugin. Even obtaining
    // app.autolaunch() would panic because its managed state is absent.
    preferences.launch_at_login = if isolated {
        false
    } else {
        read_system_login()?
    };
    Ok(preferences)
}

#[tauri::command]
pub fn update_desktop_preferences(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: DesktopPreferences,
) -> ApiResult<DesktopPreferences> {
    let mut previous = state
        .store
        .load_desktop_preferences()
        .map_err(ApiError::storage)?;
    previous.launch_at_login = app.autolaunch().is_enabled().map_err(ApiError::storage)?;

    let autostart_changed = request.launch_at_login != previous.launch_at_login;
    if autostart_changed {
        apply_autostart(&app, request.launch_at_login)?;
    }
    if let Err(error) = apply_desktop_preferences(&window, request) {
        if autostart_changed {
            let _ = apply_autostart(&app, previous.launch_at_login);
        }
        return Err(ApiError::storage(error));
    }
    match state.store.save_desktop_preferences(request) {
        Ok(saved) => Ok(saved),
        Err(error) => {
            if autostart_changed {
                let _ = apply_autostart(&app, previous.launch_at_login);
            }
            let _ = apply_desktop_preferences(&window, previous);
            Err(ApiError::storage(error))
        }
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> ApiResult<()> {
    if enabled {
        app.autolaunch().enable().map_err(ApiError::storage)
    } else {
        app.autolaunch().disable().map_err(ApiError::storage)
    }
}

#[tauri::command]
pub fn capture_status(state: State<'_, DesktopState>) -> ApiResult<CaptureStatus> {
    state.capture.status().map_err(ApiError::storage)
}

#[tauri::command]
pub fn pause_capture(
    state: State<'_, DesktopState>,
    request: PauseRequest,
) -> ApiResult<CaptureStatus> {
    if request.minutes == Some(0) {
        return Err(ApiError::invalid("pause duration must be positive"));
    }
    state
        .capture
        .pause(request.minutes)
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn resume_capture(state: State<'_, DesktopState>) -> ApiResult<CaptureStatus> {
    state.capture.resume().map_err(ApiError::storage)
}

#[tauri::command]
pub async fn set_preview_window(window: WebviewWindow, open: bool) -> ApiResult<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let main = window.clone();
    window
        .run_on_main_thread(move || {
            let _ = sender.send(crate::window::set_preview_open(&main, open));
        })
        .map_err(ApiError::storage)?;
    receiver
        .await
        .map_err(ApiError::storage)?
        .map_err(ApiError::storage)
}

#[tauri::command]
pub fn hide_window(window: WebviewWindow) -> ApiResult<()> {
    hide_main_window(&window).map_err(ApiError::storage)
}

#[tauri::command]
pub fn show_window(window: WebviewWindow) -> ApiResult<()> {
    show_main_window(&window).map_err(ApiError::storage)
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_direct_paste_permission, mcp_connection_configuration, parse_link_preview_url,
        parse_shared_conflict_resolution, parse_sync_conflict_resolution, read_desktop_preferences,
    };
    use paste_storage::SharedConflictResolution;
    use paste_storage::SyncConflictResolution;
    use std::path::Path;

    #[test]
    fn pdf_thumbnail_cache_preserves_original_document_export_and_outbox() {
        use paste_domain::{
            CaptureFlags, CapturedItem, CapturedRepresentation, RepresentationKind,
            SourceApplication,
        };
        let store = paste_storage::SqliteStore::open_in_memory().expect("synthetic store");
        let original = CapturedRepresentation {
            kind: RepresentationKind::Pdf,
            native_type: Some("com.adobe.pdf".into()),
            mime_type: Some("application/pdf".into()),
            file_name: Some("synthetic.pdf".into()),
            bytes: crate::pdf_preview::fixture::document(0, false),
        };
        let item = store
            .insert_capture(&CapturedItem {
                captured_at: chrono::Utc::now(),
                source: SourceApplication::unknown(),
                device: store.get_or_create_device("Synthetic Mac").expect("device"),
                flags: CaptureFlags::default(),
                representations: vec![original.clone()],
            })
            .expect("capture");
        let before = store
            .prepare_private_sync_batch(250)
            .expect("queue")
            .operations
            .len();
        let preview = super::load_thumbnail(&store, item.id).expect("first page");
        assert_eq!(preview.media_type, "image/png");
        assert_eq!((preview.pixel_width, preview.pixel_height), (600, 900));
        assert_eq!(
            super::load_thumbnail(&store, item.id)
                .expect("cached")
                .bytes,
            preview.bytes
        );
        assert!(
            store
                .load_cached_preview(item.id, &[99; 32])
                .expect("wrong revision")
                .is_none()
        );
        assert_eq!(
            store.load_clipboard_payload(item.id).expect("original"),
            vec![original.clone()]
        );
        assert_eq!(
            store
                .prepare_private_sync_batch(250)
                .expect("queue")
                .operations
                .len(),
            before
        );
        let exports = tempfile::tempdir().expect("synthetic exports");
        let paths = crate::drag_export::prepare_drag_files(&store, exports.path(), &[item.id])
            .expect("PDF export");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().and_then(|s| s.to_str()), Some("pdf"));
        assert_eq!(
            std::fs::read(&paths[0]).expect("export bytes"),
            original.bytes
        );
    }

    #[tokio::test]
    async fn full_pdf_preview_uses_validated_original_and_errors_without_mutating_history() {
        use paste_domain::{
            CaptureFlags, CapturedItem, CapturedRepresentation, RepresentationKind,
            SourceApplication,
        };
        for (name, bytes, valid) in [
            (
                "three-page.pdf",
                include_bytes!("../fixtures/native-preview-acceptance.pdf").as_slice(),
                true,
            ),
            (
                "locked.pdf",
                include_bytes!("../fixtures/native-preview-locked.pdf").as_slice(),
                false,
            ),
            ("invalid.pdf", b"%PDF-1.4\nnot a document".as_slice(), false),
        ] {
            let store = std::sync::Arc::new(
                paste_storage::SqliteStore::open_in_memory().expect("synthetic storage"),
            );
            let item = store
                .insert_capture(&CapturedItem {
                    captured_at: chrono::Utc::now(),
                    source: SourceApplication::unknown(),
                    device: store.get_or_create_device("Synthetic Mac").expect("device"),
                    flags: CaptureFlags::default(),
                    representations: vec![CapturedRepresentation {
                        kind: RepresentationKind::Pdf,
                        native_type: Some("com.adobe.pdf".into()),
                        mime_type: Some("application/pdf".into()),
                        file_name: Some(name.into()),
                        bytes: bytes.to_vec(),
                    }],
                })
                .expect("capture");
            let before = store
                .prepare_private_sync_batch(250)
                .expect("queue")
                .operations
                .len();
            let preview_store = std::sync::Arc::clone(&store);
            let clip_id = item.id;
            let result = tokio::task::spawn_blocking(move || {
                super::load_pdf_preview(&preview_store, clip_id)
            })
            .await
            .expect("PDF parser worker");
            if valid {
                let preview = result.expect("valid original PDF");
                assert_eq!(preview.media_type, "application/pdf");
                assert_eq!(preview.data_url, super::data_url("application/pdf", bytes));
                assert_eq!((preview.pixel_width, preview.pixel_height), (0, 0));
            } else {
                assert!(result.is_err(), "{name} must surface a preview error");
            }
            assert_eq!(
                store
                    .load_clipboard_payload(item.id)
                    .expect("unchanged payload")[0]
                    .bytes,
                bytes
            );
            assert_eq!(store.get_clip(item.id).expect("unchanged clip"), Some(item));
            assert_eq!(
                store
                    .prepare_private_sync_batch(250)
                    .expect("unchanged queue")
                    .operations
                    .len(),
                before
            );
        }
    }

    #[test]
    fn icon_requests_only_resolve_existing_clip_sources_and_deduplicate_without_writes() {
        let store = paste_storage::SqliteStore::open_in_memory().expect("synthetic store");
        let device = store.get_or_create_device("Synthetic Mac").expect("device");
        let mut ids = Vec::new();
        for text in ["one", "two"] {
            let clip = store
                .create_textual_item(
                    paste_domain::ContentKind::Text,
                    text,
                    paste_domain::SourceApplication {
                        bundle_identifier: "com.apple.TextEdit".into(),
                        display_name: "Synthetic source".into(),
                    },
                    device.clone(),
                )
                .expect("item");
            ids.push(clip.id.to_string());
        }
        let before = store
            .prepare_private_sync_batch(250)
            .expect("before")
            .operations
            .len();
        assert_eq!(
            super::source_icon_keys(&store, ids.clone()).expect("keys"),
            vec!["com.apple.TextEdit"]
        );
        assert_eq!(
            store
                .prepare_private_sync_batch(250)
                .expect("after")
                .operations
                .len(),
            before
        );
        ids.push(paste_domain::ClipId::new().to_string());
        assert!(super::source_icon_keys(&store, ids).is_err());
    }

    #[test]
    fn icon_request_bounds_and_non_clip_inputs_are_rejected() {
        let store = paste_storage::SqliteStore::open_in_memory().expect("synthetic store");
        for values in [
            vec![],
            vec!["com.apple.TextEdit".into()],
            vec!["file:///Applications/TextEdit.app".into()],
            vec![paste_domain::ClipId::new().to_string(); 17],
        ] {
            assert!(super::source_icon_keys(&store, values).is_err());
        }
    }

    #[test]
    fn isolated_desktop_preferences_never_access_the_unregistered_autostart_plugin() {
        let store = paste_storage::SqliteStore::open_in_memory().expect("synthetic store");
        let saved = paste_domain::DesktopPreferences {
            language: paste_domain::Language::Chinese,
            launch_at_login: true,
            compact_mode: true,
            screen_share_protection: true,
        };
        store
            .save_desktop_preferences(saved)
            .expect("synthetic preferences");
        let preferences = read_desktop_preferences(&store, true, || {
            panic!("must not obtain the system autostart plugin in isolation")
        })
        .expect("isolated preferences");
        assert!(!preferences.launch_at_login);
        assert!(preferences.compact_mode && preferences.screen_share_protection);
        assert_eq!(
            store.load_desktop_preferences().expect("unchanged store"),
            saved
        );
    }

    #[test]
    fn normal_desktop_preferences_use_system_login_status_and_propagate_errors() {
        let store = paste_storage::SqliteStore::open_in_memory().expect("synthetic store");
        for enabled in [true, false] {
            assert_eq!(
                read_desktop_preferences(&store, false, || Ok(enabled))
                    .expect("system status")
                    .launch_at_login,
                enabled
            );
        }
        let failure = read_desktop_preferences(&store, false, || {
            Err(super::ApiError::storage("synthetic system error"))
        })
        .expect_err("do not hide normal-mode system errors");
        assert_eq!(failure.message, "synthetic system error");
    }

    #[test]
    fn direct_paste_reports_permission_instead_of_failing_silently() {
        let error = ensure_direct_paste_permission(false).expect_err("permission should be needed");
        assert_eq!(error.code, "permission_required");
        assert!(error.message.contains("内容已复制"));
        assert!(ensure_direct_paste_permission(true).is_ok());
    }

    #[test]
    fn permission_help_identifies_the_actual_bundle_not_another_same_named_app() {
        assert_eq!(
            super::bundle_path_for_executable(Path::new(
                "/local/beta/CopyRail.app/Contents/MacOS/pasters-desktop"
            )),
            Some("/local/beta/CopyRail.app".into())
        );
        for value in [
            "/local/target/debug/pasters-desktop",
            "/local/CopyRail.app/fake/pasters-desktop",
        ] {
            assert_eq!(super::bundle_path_for_executable(Path::new(value)), None);
        }
    }

    #[test]
    fn restore_lost_focus_rejects_before_any_clipboard_write() {
        assert!(super::ensure_restore_focus(true).is_ok());
        let error = super::ensure_restore_focus(false).expect_err("unfocused invocation");
        assert!(error.message.contains("剪贴板未改动"));
    }

    #[test]
    fn only_submitted_events_report_requested_and_abort_reasons_remain_explicit() {
        use paste_platform::PasteOutcome;
        assert!(super::paste_outcome_result(PasteOutcome::Requested).expect("submitted"));
        assert!(!super::paste_outcome_result(PasteOutcome::NoTarget).expect("copy fallback"));
        for outcome in [
            PasteOutcome::SessionChanged,
            PasteOutcome::TargetUnavailable,
            PasteOutcome::FocusChanged,
            PasteOutcome::ClipboardChanged,
            PasteOutcome::PermissionDenied,
            PasteOutcome::ActivationRefused,
            PasteOutcome::ActivationTimedOut,
            PasteOutcome::PostFailed,
        ] {
            let error = super::paste_outcome_result(outcome).expect_err("not submitted");
            assert!(!error.message.is_empty());
            assert!(!error.message.contains("已粘贴"));
            if outcome == PasteOutcome::ClipboardChanged {
                assert!(
                    !error.message.contains("内容已复制"),
                    "clipboard no longer contains this operation"
                );
            }
        }
    }

    #[test]
    fn link_preview_accepts_web_urls_and_rejects_privileged_schemes() {
        assert!(parse_link_preview_url("https://pasteapp.io/help").is_ok());
        assert!(parse_link_preview_url("http://localhost:8080/path").is_ok());
        assert!(parse_link_preview_url("file:///etc/passwd").is_err());
        assert!(parse_link_preview_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn mcp_configuration_uses_stdio_and_keeps_the_token_out_of_arguments() {
        let configuration = mcp_connection_configuration(
            Path::new("/Applications/CopyRail.app/Contents/MacOS/pasters-desktop"),
            Path::new("/Users/test/Library/Application Support/io.pasters.app/history.db"),
            "secret-token",
        )
        .expect("configuration");
        let parsed: serde_json::Value =
            serde_json::from_str(&configuration).expect("valid JSON configuration");
        let server = &parsed["mcpServers"]["pasters"];
        assert_eq!(server["args"], serde_json::json!(["--mcp-stdio"]));
        assert_eq!(server["env"]["PASTERS_MCP_TOKEN"], "secret-token");
        assert!(!server["args"].to_string().contains("secret-token"));
    }

    #[test]
    fn shared_conflict_resolution_has_explicit_actions_and_rejects_private_aliases() {
        for (name, expected) in [
            ("keep_current", SharedConflictResolution::KeepCurrent),
            ("use_first", SharedConflictResolution::UseFirst),
            ("use_second", SharedConflictResolution::UseSecond),
        ] {
            assert_eq!(
                parse_shared_conflict_resolution(name).expect("explicit choice"),
                expected
            );
        }
        for invalid in ["", "keep_local", "accept_remote", "keep_both", "USE_FIRST"] {
            assert!(parse_shared_conflict_resolution(invalid).is_err());
        }
    }

    #[test]
    fn sync_conflict_resolution_rejects_ambiguous_actions() {
        assert_eq!(
            parse_sync_conflict_resolution("keep_local").expect("keep local"),
            SyncConflictResolution::KeepLocal
        );
        assert_eq!(
            parse_sync_conflict_resolution("accept_remote").expect("accept remote"),
            SyncConflictResolution::AcceptRemote
        );
        assert!(parse_sync_conflict_resolution("keep_both").is_err());
    }
}
