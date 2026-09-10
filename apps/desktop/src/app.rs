use crate::i18n::{localized_format, t};
use crate::workspace_protocol::{WorkspaceContent, WorkspaceKey, WorkspaceSnapshot};
use std::collections::{HashMap, HashSet};

use crate::context_action::ContextAction;
use crate::drag_drop::{self, DropTarget};
use crate::drag_gesture::CardPress;
use crate::gesture_trace::{self, GesturePhase, GestureTrace};
use crate::search::{self, SearchContext, SearchKeyAction};
use gloo_timers::future::TimeoutFuture;
use js_sys::Promise;
use leptos::{ev, html, prelude::*, task::spawn_local};
use paste_domain::{
    CapturePreferences, ClipId, ClipItem, ContentKind, DesktopPreferences, DeviceId, Pinboard,
    PinboardId, RetentionPolicy, SearchFacets,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const FILTER_KINDS: [ContentKind; 7] = [
    ContentKind::Text,
    ContentKind::RichText,
    ContentKind::Link,
    ContentKind::Image,
    ContentKind::File,
    ContentKind::Pdf,
    ContentKind::Color,
];

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    fn invoke_js(command: &str, args: JsValue) -> Promise;
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = window, js_name = copyrailWindowRole)]
    fn window_role() -> String;
    #[wasm_bindgen(js_namespace = window, js_name = copyrailWorkspacePaint)]
    fn workspace_paint() -> Promise;
    #[wasm_bindgen(js_namespace = window, js_name = copyrailDispatchKey)]
    fn dispatch_workspace_key(key: &str, shift: bool, meta: bool);
}
#[derive(Deserialize)]
struct WorkspaceEvent {
    payload: WorkspaceSnapshot,
}
#[derive(Deserialize)]
struct WorkspaceKeyEvent {
    payload: WorkspaceKey,
}
#[derive(Serialize)]
struct WorkspaceReadyArgs {
    revision: u64,
}

#[derive(Serialize)]
struct CommandArgs<T> {
    request: T,
}

#[derive(Deserialize)]
struct NativeEditEvent {
    payload: String,
}

#[derive(Serialize)]
struct NativeTextActionArgs {
    action: String,
}

#[derive(Serialize)]
struct PreviewWindowArgs {
    open: bool,
}

#[derive(Serialize)]
struct HistoryRequest {
    limit: u32,
    offset: u32,
}

#[derive(Serialize)]
struct SearchRequest {
    text: String,
    pinboard_id: Option<String>,
    content_kinds: Vec<ContentKind>,
    source_bundle_ids: Vec<String>,
    device_ids: Vec<String>,
    copied_after_ms: Option<i64>,
    copied_before_ms: Option<i64>,
    limit: u32,
    offset: u32,
}

#[derive(Serialize)]
struct PauseRequest {
    minutes: Option<u32>,
}

#[derive(Serialize)]
struct CloudSyncEnabledRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct ResolveSyncConflictRequest {
    conflict_id: String,
    resolution: String,
}

#[derive(Serialize)]
struct ResolveSharedConflictRequest {
    conflict_id: String,
    current_operation_id: String,
    resolution: String,
}

#[derive(Serialize)]
struct PinboardRequest {
    name: String,
    color: String,
}

#[derive(Serialize)]
struct UpdatePinboardRequest {
    pinboard_id: String,
    name: String,
    color: String,
}

#[derive(Serialize)]
struct ReorderPinboardsRequest {
    pinboard_ids: Vec<String>,
}

#[derive(Serialize)]
struct MovePinboardItemRequest {
    pinboard_id: String,
    clip_id: String,
    direction: i8,
}

#[derive(Serialize)]
struct PlacePinboardClipsRequest {
    pinboard_id: String,
    clip_ids: Vec<String>,
    anchor: Option<String>,
    after: bool,
}

#[derive(Serialize)]
struct DeletePinboardRequest {
    pinboard_id: String,
}

#[derive(Serialize)]
struct CreateTextualItemRequest {
    kind: ContentKind,
    value: String,
}

#[derive(Serialize)]
struct UpdateTextualItemRequest {
    clip_id: String,
    kind: ContentKind,
    title: String,
    value: String,
}

#[derive(Serialize)]
struct RenameRequest {
    clip_id: String,
    title: String,
}

#[derive(Serialize)]
struct RestoreRequest {
    clip_id: String,
    plain_text: bool,
    paste: bool,
}

#[derive(Serialize)]
struct RestoreManyRequest {
    clip_ids: Vec<String>,
    plain_text: bool,
    paste: bool,
}

#[derive(Serialize)]
struct ClipsRequest {
    clip_ids: Vec<String>,
}

#[derive(Serialize)]
struct ClipContextMenuRequest {
    clip_ids: Vec<String>,
    x: f64,
    y: f64,
}

#[derive(Deserialize)]
struct ClipContextMenuChoice {
    choice: ContextAction,
    item: Option<ClipItem>,
}

#[derive(Serialize)]
struct ClipRequest {
    clip_id: String,
}

#[derive(Serialize)]
struct PreviewRequest {
    clip_id: String,
}

#[derive(Serialize)]
struct RotateImageRequest {
    clip_id: String,
    direction: i8,
}

#[derive(Serialize)]
struct PinClipsRequest {
    pinboard_id: String,
    clip_ids: Vec<String>,
}

#[derive(Serialize)]
struct McpEnabledRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct CreateMcpConnectionRequest {
    display_name: String,
}

#[derive(Serialize)]
struct RevokeMcpConnectionRequest {
    client_id: String,
}

#[derive(Serialize)]
struct EmptyArgs {}

#[derive(Serialize)]
struct GestureTraceRequest {
    events: Vec<GestureTrace>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureStatus {
    #[serde(default)]
    isolated: bool,
    paused: bool,
    paused_until_ms: Option<i64>,
    last_error: Option<String>,
    #[serde(default)]
    control_pending: Option<String>,
    #[serde(default)]
    revision: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreResult {
    paste_requested: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResult {
    media_type: String,
    data_url: String,
    pixel_width: u32,
    pixel_height: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceIconResult {
    bundle_identifier: String,
    data_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OcrResult {
    item: ClipItem,
    line_count: usize,
    character_count: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DragExportResult {
    item_count: usize,
}

#[derive(Serialize)]
struct StartDragRequest {
    clip_ids: Vec<String>,
    feedback: crate::drag_feedback::DragLayout,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupActionResult {
    path: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionStatus {
    accessibility_trusted: bool,
    #[serde(default)]
    app_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutStatus {
    isolated: bool,
    registered: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    enabled: bool,
    local_outbox_ready: bool,
    cloud_transport_configured: bool,
    syncing: bool,
    last_success_at_ms: Option<i64>,
    pending_changes: u64,
    pending_conflicts: u64,
    pending_dependencies: usize,
    #[serde(default)]
    pending_shared_downloads: usize,
    #[serde(default)]
    pending_shared_conflicts: usize,
    blocked_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncConflictView {
    id: String,
    local_title: String,
    remote_title: String,
    remote_would_win: bool,
    created_at_ms: i64,
}

#[component]
fn SyncConflictList(
    conflicts: ReadSignal<Vec<SyncConflictView>>,
    resolving: ReadSignal<HashSet<String>>,
    on_resolve: Callback<(String, String)>,
) -> impl IntoView {
    move || {
        let items = conflicts.get();
        (!items.is_empty()).then(|| view! {
            <div class="sync-conflict-list" aria-label=move || t("待处理的同步冲突")>
                {items.into_iter().map(|conflict| {
                    let keep_id = conflict.id.clone();
                    let accept_id = conflict.id.clone();
                    let busy_id = conflict.id.clone();
                    let keep_busy_id = conflict.id.clone();
                    let accept_busy_id = conflict.id;
                    let ordering = if conflict.remote_would_win { t("自动顺序原本偏向另一台设备") }
                        else { t("自动顺序原本偏向此 Mac") };
                    view! {
                        <article class="sync-conflict-card" aria-busy=move || resolving.get().contains(&busy_id).to_string()>
                            <div class="sync-conflict-copy">
                                <strong>{move || localized_format!("并发编辑 · {}", "Concurrent edit · {}", relative_timestamp_ms(conflict.created_at_ms))}</strong>
                                <span>{ordering}</span>
                                <dl>
                                    <div><dt>{move || t("此 Mac")}</dt><dd>{conflict.local_title}</dd></div>
                                    <div><dt>{move || t("另一台设备")}</dt><dd>{conflict.remote_title}</dd></div>
                                </dl>
                            </div>
                            <div class="sync-conflict-actions">
                                <button type="button" disabled=move || resolving.get().contains(&keep_busy_id)
                                    on:click=move |_| on_resolve.run((keep_id.clone(), "keep_local".into()))>{move || t("保留此 Mac")}</button>
                                <button class="accept-remote" type="button" disabled=move || resolving.get().contains(&accept_busy_id)
                                    on:click=move |_| on_resolve.run((accept_id.clone(), "accept_remote".into()))>{move || t("采用另一台设备")}</button>
                            </div>
                        </article>
                    }
                }).collect_view()}
            </div>
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedConflictVersionView {
    title: String,
    preview: String,
    device_name: String,
    deleted: bool,
    timestamp_ms: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedConflictView {
    id: String,
    pinboard_name: String,
    can_resolve: bool,
    current_operation_id: String,
    current: SharedConflictVersionView,
    first: SharedConflictVersionView,
    second: SharedConflictVersionView,
}

#[component]
fn SharedConflictVersion(label: &'static str, version: SharedConflictVersionView) -> impl IntoView {
    let title = version.title;
    let body = move || {
        if version.deleted {
            t("此版本删除了该内容。采用后会同步删除状态。").to_owned()
        } else if version.preview.is_empty() {
            t("非文本内容；采用时会保留该版本的原始格式和附件。").to_owned()
        } else {
            version.preview.clone()
        }
    };
    view! {
        <details class="shared-conflict-version">
            <summary>{move || format!("{} · {}{}", t(label), title, if version.deleted { t("（删除状态）") } else { "" })}</summary>
            <small>{move || format!("{} · {}", version.device_name, relative_timestamp_ms(version.timestamp_ms))}</small>
            <p>{body}</p>
        </details>
    }
}

#[component]
fn SharedConflictList(
    conflicts: ReadSignal<Vec<SharedConflictView>>,
    resolving: ReadSignal<HashSet<String>>,
    on_resolve: Callback<(String, String, String)>,
) -> impl IntoView {
    view! {
        <Show when=move || !conflicts.get().is_empty()>
            <section class="sync-conflict-list shared-conflict-list" aria-label=move || t("共享内容冲突")>
                <strong>{move || t("共享内容冲突")}</strong>
                <span>{move || t("查看当前内容及两份保留版本。选择会生成一个新版本；已移出的私人原记录不会被恢复或覆盖。每次显示最多 50 项。")}</span>
                <For each=move || conflicts.get()
                    key=|c| (c.id.clone(), c.current_operation_id.clone(), c.can_resolve, c.pinboard_name.clone())
                    children=move |conflict| {
                        let busy_id = conflict.id.clone();
                        let readonly = !conflict.can_resolve;
                        let choices = [
                            ("keep_current", if conflict.current.deleted { "保留当前删除" } else { "保留当前内容" }),
                            ("use_first", if conflict.first.deleted { "采用版本 A（删除）" } else { "采用版本 A" }),
                            ("use_second", if conflict.second.deleted { "采用版本 B（删除）" } else { "采用版本 B" }),
                        ];
                        let actions = choices.into_iter().map(|(choice, label)| {
                            let id = conflict.id.clone();
                            let busy = id.clone();
                            let revision = conflict.current_operation_id.clone();
                            view! { <button type="button" disabled=move || readonly || resolving.get().contains(&busy)
                                on:click=move |_| on_resolve.run((id.clone(), revision.clone(), choice.into()))>{move || t(label)}</button> }
                        }).collect_view();
                        view! {
                            <article class="sync-conflict-card" aria-busy=move || resolving.get().contains(&busy_id).to_string()>
                                <strong>{conflict.pinboard_name}</strong>
                                <SharedConflictVersion label="当前" version=conflict.current />
                                <SharedConflictVersion label="版本 A" version=conflict.first />
                                <SharedConflictVersion label="版本 B" version=conflict.second />
                                {readonly.then(|| view! { <span>{move || t("此共享板为只读，只有可编辑成员可以提交选择。")}</span> })}
                                <div class="sync-conflict-actions shared-conflict-actions">{actions}</div>
                            </article>
                        }
                    } />
            </section>
        </Show>
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpClientView {
    id: String,
    display_name: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpAccessStatus {
    enabled: bool,
    clients: Vec<McpClientView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpConnectionResult {
    status: McpAccessStatus,
    configuration: String,
}

#[derive(Debug, Deserialize)]
struct ClientApiError {
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryPositionResult {
    position: u32,
}

async fn invoke<T: DeserializeOwned>(command: &str, args: &impl Serialize) -> Result<T, String> {
    let args = serde_wasm_bindgen::to_value(args).map_err(|error| error.to_string())?;
    let value = JsFuture::from(invoke_js(command, args))
        .await
        .map_err(js_error)?;
    serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())
}

async fn write_clip(
    clip_id: ClipId,
    plain_text: bool,
    paste: bool,
) -> Result<RestoreResult, String> {
    invoke(
        "restore_clip",
        &CommandArgs {
            request: RestoreRequest {
                clip_id: clip_id.to_string(),
                plain_text,
                paste,
            },
        },
    )
    .await
}

async fn restore_clip(clip_id: ClipId, plain_text: bool) -> Result<RestoreResult, String> {
    write_clip(clip_id, plain_text, true).await
}

async fn write_clips(
    clip_ids: Vec<ClipId>,
    plain_text: bool,
    paste: bool,
) -> Result<RestoreResult, String> {
    if let [clip_id] = clip_ids.as_slice() {
        return write_clip(*clip_id, plain_text, paste).await;
    }
    invoke(
        "restore_clips",
        &CommandArgs {
            request: RestoreManyRequest {
                clip_ids: clip_ids
                    .into_iter()
                    .map(|clip_id| clip_id.to_string())
                    .collect(),
                plain_text,
                paste,
            },
        },
    )
    .await
}

async fn restore_clips(clip_ids: Vec<ClipId>, plain_text: bool) -> Result<RestoreResult, String> {
    write_clips(clip_ids, plain_text, true).await
}

async fn open_link_preview(clip_id: ClipId) -> Result<(), String> {
    invoke(
        "open_link_preview",
        &CommandArgs {
            request: ClipRequest {
                clip_id: clip_id.to_string(),
            },
        },
    )
    .await
}

fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| {
        serde_wasm_bindgen::from_value::<ClientApiError>(value)
            .map(|error| error.message)
            .unwrap_or_else(|_| t("桌面服务暂时不可用").into())
    })
}

#[component]
pub fn App() -> impl IntoView {
    let role = window_role();
    let auxiliary = role == "workspace";
    let native_rail = role == "main";
    crate::i18n::init();
    let (clips, set_clips) = signal(Vec::<ClipItem>::new());
    let (pinboards, set_pinboards) = signal(Vec::<Pinboard>::new());
    let (active_pinboard, set_active_pinboard) = signal(None::<PinboardId>);
    let (query, set_query) = signal(String::new());
    let (selected, set_selected) = signal(0_usize);
    let (selected_ids, set_selected_ids) = signal(HashSet::<ClipId>::new());
    let (selection_anchor, set_selection_anchor) = signal(0_usize);
    let (stack, set_stack) = signal(Vec::<ClipId>::new());
    let (status, set_status) = signal(None::<CaptureStatus>);
    let capture_request_busy = RwSignal::new(None::<&'static str>);
    let capture_isolated = Memo::new(move |_| status.get().is_some_and(|value| value.isolated));
    let apply_capture_status = Callback::new(move |incoming: CaptureStatus| {
        set_status.update(|current| {
            if current
                .as_ref()
                .is_none_or(|value| incoming.revision >= value.revision)
            {
                *current = Some(incoming);
            }
        });
    });
    // Every webview needs authoritative worker confirmations, including the
    // detached settings panel. Do not couple status to history loading, which
    // is deliberately skipped in that panel and suspended during rail gestures.
    spawn_local(async move {
        loop {
            if let Ok(current) = invoke::<CaptureStatus>("capture_status", &EmptyArgs {}).await {
                apply_capture_status.run(current);
            }
            TimeoutFuture::new(500).await;
        }
    });
    let pending_capture_control = move || {
        status
            .get()
            .and_then(|value| value.control_pending)
            .or_else(|| capture_request_busy.get().map(str::to_owned))
    };
    let capture_status_visible = Memo::new(move |_| {
        status.get().is_some_and(|value| value.paused) || pending_capture_control().is_some()
    });
    let (error, set_error) = signal(None::<String>);
    let (notice, set_notice) = signal(None::<String>);
    let notice_epoch = RwSignal::new(0_u64);
    Effect::new(move |_| {
        let message = notice.get();
        let epoch = notice_epoch.get_untracked().wrapping_add(1);
        notice_epoch.set(epoch);
        if message.is_some() {
            set_timeout(
                move || {
                    if notice_epoch.try_get_untracked() == Some(epoch) {
                        let _ = set_notice.try_set(None);
                    }
                },
                std::time::Duration::from_secs(4),
            );
        }
    });
    let drop_target = RwSignal::new(None::<DropTarget>);
    let native_dragging = RwSignal::new(false);
    let native_feedback_active = RwSignal::new(false);
    let drag_lifecycle = StoredValue::new_local(std::cell::RefCell::new(
        crate::drag_lifecycle::DragLifecycle::default(),
    ));
    let drag_layouts = StoredValue::new_local(std::cell::RefCell::new(
        crate::drag_lifecycle::LayoutPublisher::default(),
    ));
    let card_press = RwSignal::new(None::<CardPress>);
    let context_menu_open = RwSignal::new(false);
    let trace_buffer = StoredValue::new(Vec::<GestureTrace>::new());
    let trace_flush_pending = StoredValue::new(false);
    let trace_gesture = Callback::new(move |event: GestureTrace| {
        if !status.get_untracked().is_some_and(|state| state.isolated) {
            return;
        }
        trace_buffer.update_value(|events| {
            if events.len() < 64 {
                events.push(event);
            }
        });
        if trace_flush_pending.get_value() {
            return;
        }
        trace_flush_pending.set_value(true);
        // Batch after the initiating gesture; never await logging before drag IPC.
        spawn_local(async move {
            TimeoutFuture::new(250).await;
            let events = trace_buffer.get_value();
            trace_buffer.set_value(Vec::new());
            trace_flush_pending.set_value(false);
            let _ = invoke::<()>(
                "trace_native_gesture",
                &CommandArgs {
                    request: GestureTraceRequest { events },
                },
            )
            .await;
        });
    });
    let context_menu_callback = StoredValue::new(None::<Callback<(ClipId, f64, f64)>>);
    let placement_busy = RwSignal::new(false);
    let order_revision = RwSignal::new(0_u64);
    let tab_drag = RwSignal::new(None::<PinboardId>);
    let tab_hover = RwSignal::new(None::<(PinboardId, bool)>);
    let tab_pointer = RwSignal::new(None::<(PinboardId, i32, i32)>);
    let tab_click_suppressed = RwSignal::new(false);
    let (settings_open, set_settings_open) = signal(false);
    let settings_tab = RwSignal::new("general");
    let (pinboard_creator_open, set_pinboard_creator_open) = signal(false);
    let (pinboard_editor_open, set_pinboard_editor_open) = signal(false);
    let (pin_menu_open, set_pin_menu_open) = signal(false);
    let (filter_menu_open, set_filter_menu_open) = signal(false);
    let (active_kind, set_active_kind) = signal(None::<ContentKind>);
    let (active_source, set_active_source) = signal(None::<String>);
    let (active_device, set_active_device) = signal(None::<DeviceId>);
    let (date_days, set_date_days) = signal(None::<i64>);
    let (search_facets, set_search_facets) = signal(SearchFacets::default());
    let (history_offset, set_history_offset) = signal(0_u32);
    let (jump_target, set_jump_target) = signal(None::<ClipId>);
    let (new_pinboard_name, set_new_pinboard_name) = signal(String::new());
    let (new_pinboard_color, set_new_pinboard_color) = signal("#ff9500".to_owned());
    let (edit_pinboard_name, set_edit_pinboard_name) = signal(String::new());
    let (edit_pinboard_color, set_edit_pinboard_color) = signal("#ff9500".to_owned());
    let (retention_days, set_retention_days) = signal(String::new());
    let (retention_items, set_retention_items) = signal(String::new());
    let (excluded_apps, set_excluded_apps) = signal(String::new());
    let (desktop_preferences, set_desktop_preferences) = signal(DesktopPreferences::default());
    let settings_saving = RwSignal::new(false);
    let appearance_saving = RwSignal::new(false);
    let appearance_error = RwSignal::new(false);
    let change_transparency = move |event: ev::Event| {
        let Ok(next) = event_target_value(&event).parse::<u8>() else {
            return;
        };
        if next > 100 || settings_saving.get_untracked() {
            return;
        }
        let mut previous = desktop_preferences.get_untracked().background_transparency;
        set_desktop_preferences.update(|draft| draft.background_transparency = next);
        appearance_error.set(false);
        if appearance_saving.get_untracked() {
            return;
        }
        appearance_saving.set(true);
        spawn_local(async move {
            loop {
                TimeoutFuture::new(100).await;
                let request = desktop_preferences.get_untracked().background_transparency;
                match invoke::<u8>("set_background_transparency", &CommandArgs { request }).await {
                    Ok(saved) => previous = saved,
                    Err(_) => {
                        set_desktop_preferences
                            .update(|draft| draft.background_transparency = previous);
                        appearance_error.set(true);
                        break;
                    }
                }
                if desktop_preferences.get_untracked().background_transparency == request {
                    break;
                }
            }
            appearance_saving.set(false);
        });
    };
    let language_saving = RwSignal::new(false);
    let language_error = RwSignal::new(false);
    let change_language = move |event: ev::Event| {
        if language_saving.get_untracked() || settings_saving.get_untracked() {
            return;
        }
        let next = match event_target_value(&event).as_str() {
            "system" => paste_domain::LanguagePreference::System,
            "en" => paste_domain::LanguagePreference::English,
            "zh-CN" => paste_domain::LanguagePreference::Chinese,
            _ => return,
        };
        language_saving.set(true);
        language_error.set(false);
        spawn_local(async move {
            match invoke::<paste_domain::LanguageSettings>(
                "set_language",
                &CommandArgs { request: next },
            )
            .await
            {
                Ok(saved) => {
                    set_desktop_preferences.update(|draft| draft.language = saved.preference);
                    crate::i18n::set_language(saved.effective);
                }
                Err(_) => language_error.set(true),
            }
            language_saving.set(false);
        });
    };
    let (applied_compact, set_applied_compact) = signal(false);
    let (previews, set_previews) = signal(HashMap::<ClipId, PreviewResult>::new());
    let (preview_loading, set_preview_loading) = signal(HashSet::<ClipId>::new());
    let preview_attempts = RwSignal::new(HashMap::<ClipId, ([u8; 32], f64)>::new());
    let source_icons = RwSignal::new(crate::source_icon::IconCache::<String>::default());
    let icons_loading = RwSignal::new(false);
    let (preview_open, set_preview_open) = signal(None::<ClipId>);
    let preview_visible = Memo::new(move |_| {
        preview_open
            .get()
            .is_some_and(|id| clips.with(|items| items.iter().any(|item| item.id == id)))
    });
    let expanded_visible = Memo::new(move |_| preview_visible.get() || settings_open.get());
    let viewport_height = RwSignal::new(
        window()
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(248.0),
    );
    let dock_height = RwSignal::new(viewport_height.get_untracked());
    let preview_frame_pending = RwSignal::new(false);
    // A frontend reload may occur while the native window is still expanded.
    let preview_frame_applied = RwSignal::new(None::<bool>);
    // Keep the rail's painted surface identical while AppKit and WebKit catch
    // up with one another. Only ordinary user resizes change its stored height.
    let resize_listener = window_event_listener(ev::resize, move |_| {
        if let Ok(height) = window().inner_height()
            && let Some(height) = height.as_f64()
        {
            viewport_height.set(height);
            if !expanded_visible.get_untracked()
                && !preview_frame_pending.get_untracked()
                && preview_frame_applied.get_untracked() != Some(true)
            {
                dock_height.set(height);
            }
        }
    });
    on_cleanup(move || resize_listener.remove());
    let workspace_ready = Memo::new(move |_| {
        if auxiliary {
            return expanded_visible.get();
        }
        expanded_visible.get()
            && preview_frame_applied.get() == Some(true)
            && viewport_height.get() > dock_height.get() + 100.0
    });
    Effect::new(move |_| {
        if auxiliary || native_rail {
            return;
        }
        let desired = expanded_visible.get();
        if preview_frame_pending.get_untracked()
            || Some(desired) == preview_frame_applied.get_untracked()
        {
            return;
        }
        if desired
            && preview_frame_applied.get_untracked() != Some(true)
            && let Ok(height) = window().inner_height()
            && let Some(height) = height.as_f64()
        {
            dock_height.set(height);
        }
        preview_frame_pending.set(true);
        spawn_local(async move {
            // Serialize/coalesce rapid open/close requests. An older expansion
            // must never overtake closing the preview after a slow IPC response.
            loop {
                let desired = expanded_visible.get_untracked();
                match invoke::<()>("set_preview_window", &PreviewWindowArgs { open: desired }).await
                {
                    Ok(()) => preview_frame_applied.set(Some(desired)),
                    Err(message) => {
                        set_error.set(Some(message));
                        break;
                    }
                }
                if expanded_visible.get_untracked() == desired {
                    break;
                }
            }
            preview_frame_pending.set(false);
        });
    });
    let (image_edit_loading, set_image_edit_loading) = signal(HashSet::<ClipId>::new());
    let (ocr_loading, set_ocr_loading) = signal(HashSet::<ClipId>::new());
    let (permission_status, set_permission_status) = signal(None::<PermissionStatus>);
    let (shortcut_status, set_shortcut_status) = signal(None::<ShortcutStatus>);
    let (shortcut_retrying, set_shortcut_retrying) = signal(false);
    let (shortcut_request_error, set_shortcut_request_error) = signal(None::<String>);
    let shortcut_epoch = RwSignal::new(0_u64);
    let shortcut_section = NodeRef::<html::Section>::new();
    let permission_section = NodeRef::<html::Section>::new();
    let settings_focus_target = RwSignal::new(None::<&'static str>);
    Effect::new(move |_| {
        if !settings_open.get() {
            settings_focus_target.set(None);
            return;
        }
        if workspace_ready.get() {
            let section = match settings_focus_target.get() {
                Some("shortcuts") => shortcut_section.get(),
                Some("permission") => permission_section.get(),
                _ => None,
            };
            if let Some(section) = section {
                section.scroll_into_view_with_bool(true);
                let _ = section.focus();
                settings_focus_target.set(None);
            }
        }
    });
    let refresh_shortcut = Callback::new(move |()| {
        if shortcut_retrying.get_untracked() {
            return;
        }
        let epoch = shortcut_epoch.get_untracked().wrapping_add(1);
        shortcut_epoch.set(epoch);
        spawn_local(async move {
            let result = invoke::<ShortcutStatus>("get_shortcut_status", &EmptyArgs {}).await;
            if shortcut_epoch.get_untracked() != epoch {
                return;
            }
            match result {
                Ok(current) => {
                    set_shortcut_status.set(Some(current));
                    set_shortcut_request_error.set(None);
                }
                Err(message) => set_shortcut_request_error.set(Some(message)),
            }
        });
    });
    let retry_shortcut = move |_| {
        if shortcut_retrying.get_untracked()
            || shortcut_status
                .get_untracked()
                .is_none_or(|value| value.isolated || value.registered)
        {
            return;
        }
        shortcut_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
        set_shortcut_retrying.set(true);
        set_shortcut_request_error.set(None);
        spawn_local(async move {
            match invoke::<ShortcutStatus>("retry_shortcut", &EmptyArgs {}).await {
                Ok(current) => set_shortcut_status.set(Some(current)),
                Err(message) => set_shortcut_request_error.set(Some(message)),
            }
            set_shortcut_retrying.set(false);
        });
    };
    let (sync_status, set_sync_status) = signal(None::<SyncStatus>);
    let (sync_conflicts, set_sync_conflicts) = signal(Vec::<SyncConflictView>::new());
    let (shared_conflicts, set_shared_conflicts) = signal(Vec::<SharedConflictView>::new());
    let (resolving_shared_conflicts, set_resolving_shared_conflicts) =
        signal(HashSet::<String>::new());
    let (resolving_conflicts, set_resolving_conflicts) = signal(HashSet::<String>::new());
    let (mcp_status, set_mcp_status) = signal(None::<McpAccessStatus>);
    let (mcp_client_name, set_mcp_client_name) = signal(String::new());
    let (mcp_connection_config, set_mcp_connection_config) = signal(None::<String>);
    let (content_editor_open, set_content_editor_open) = signal(false);
    let (content_editor_id, set_content_editor_id) = signal(None::<ClipId>);
    let (content_editor_kind, set_content_editor_kind) = signal(ContentKind::Text);
    let (content_editor_title, set_content_editor_title) = signal(String::new());
    let (content_editor_value, set_content_editor_value) = signal(String::new());
    let (content_editor_rename_only, set_content_editor_rename_only) = signal(false);
    let content_editor_dialog = NodeRef::<html::Dialog>::new();
    let editor_return_focus = StoredValue::new(None::<web_sys::HtmlElement>);
    let search_input = NodeRef::<html::Input>::new();
    let results_view = NodeRef::<html::Section>::new();
    let results_have_focus = RwSignal::new(false);
    let search_context = Memo::new(move |_| SearchContext {
        text: query.get(),
        board: active_pinboard.get(),
        kind: active_kind.get(),
        source: active_source.get(),
        device: active_device.get(),
        days: date_days.get(),
        history_offset: history_offset.get(),
    });
    let search_active = Memo::new(move |_| search_context.get().is_search());
    let loaded_context = RwSignal::new(None::<SearchContext>);
    let results_pending =
        Memo::new(move |_| loaded_context.get().as_ref() != Some(&search_context.get()));
    // A changed request must not leave old cards available to click or paste.
    Effect::new(move |_| {
        let _ = search_context.get();
        set_clips.set(Vec::new());
        set_selected.set(0);
        set_selected_ids.set(HashSet::new());
        set_selection_anchor.set(0);
        set_error.set(None);
    });

    Effect::new(move |_| {
        if !settings_open.get() {
            set_mcp_connection_config.set(None);
        }
    });

    Effect::new(move |_| {
        if settings_open.get() {
            spawn_local(async move {
                loop {
                    TimeoutFuture::new(2_000).await;
                    if !settings_open.get_untracked() {
                        break;
                    }
                    if let Ok(current) =
                        invoke::<SyncStatus>("get_sync_status", &EmptyArgs {}).await
                    {
                        set_sync_status.set(Some(current));
                    }
                    if let Ok(conflicts) =
                        invoke::<Vec<SyncConflictView>>("list_sync_conflicts", &EmptyArgs {}).await
                    {
                        set_sync_conflicts.set(conflicts);
                    }
                    if let Ok(conflicts) =
                        invoke::<Vec<SharedConflictView>>("list_shared_conflicts", &EmptyArgs {})
                            .await
                    {
                        set_shared_conflicts.set(conflicts);
                    }
                }
            });
        }
    });

    spawn_local(async move {
        if let Ok(settings) =
            invoke::<paste_domain::LanguageSettings>("get_language_settings", &EmptyArgs {}).await
        {
            crate::i18n::set_language(settings.effective);
            set_desktop_preferences.update(|draft| draft.language = settings.preference);
        }
        refresh_shortcut.run(());
        if let Ok(preferences) =
            invoke::<CapturePreferences>("get_capture_preferences", &EmptyArgs {}).await
        {
            set_retention_days.set(optional_number(preferences.retention.max_age_days));
            set_retention_items.set(optional_number(preferences.retention.max_unpinned_items));
            set_excluded_apps.set(preferences.excluded_bundle_ids.join("\n"));
        }
        if let Ok(preferences) =
            invoke::<DesktopPreferences>("get_desktop_preferences", &EmptyArgs {}).await
        {
            set_desktop_preferences.set(preferences);
            set_applied_compact.set(preferences.compact_mode);
            if let Ok(settings) =
                invoke::<paste_domain::LanguageSettings>("get_language_settings", &EmptyArgs {})
                    .await
            {
                crate::i18n::set_language(settings.effective);
            }
        }
        if let Ok(current) =
            invoke::<PermissionStatus>("get_permission_status", &EmptyArgs {}).await
        {
            set_permission_status.set(Some(current));
        }
        if auxiliary {
            return;
        }
        loop {
            if native_dragging.get_untracked()
                || context_menu_open.get_untracked()
                || card_press.get_untracked().is_some()
                || tab_drag.get_untracked().is_some()
                || placement_busy.get_untracked()
            {
                TimeoutFuture::new(100).await;
                continue;
            }
            let requested_revision = order_revision.get_untracked();
            let requested_context = search_context.get_untracked();
            let text = requested_context.text.clone();
            let pinboard_id = requested_context.scoped_board().map(|id| id.to_string());
            let content_kinds = requested_context.kind.into_iter().collect::<Vec<_>>();
            let source_bundle_ids = requested_context
                .source
                .clone()
                .into_iter()
                .collect::<Vec<_>>();
            let device_ids = requested_context
                .device
                .map(|id| id.to_string())
                .into_iter()
                .collect::<Vec<_>>();
            let copied_after_ms = requested_context
                .days
                .map(|days| (chrono::Utc::now() - chrono::Duration::days(days)).timestamp_millis());
            let has_filters = pinboard_id.is_some()
                || !content_kinds.is_empty()
                || !source_bundle_ids.is_empty()
                || !device_ids.is_empty()
                || copied_after_ms.is_some();
            let loaded = if text.trim().is_empty() && !has_filters {
                invoke::<Vec<ClipItem>>(
                    "list_history",
                    &CommandArgs {
                        request: HistoryRequest {
                            limit: 200,
                            offset: requested_context.history_offset,
                        },
                    },
                )
                .await
            } else {
                invoke::<Vec<ClipItem>>(
                    "search_history",
                    &CommandArgs {
                        request: SearchRequest {
                            text,
                            pinboard_id,
                            content_kinds,
                            source_bundle_ids,
                            device_ids,
                            copied_after_ms,
                            copied_before_ms: None,
                            limit: 200,
                            offset: 0,
                        },
                    },
                )
                .await
            };
            if native_dragging.get_untracked()
                || context_menu_open.get_untracked()
                || card_press.get_untracked().is_some()
                || tab_drag.get_untracked().is_some()
                || placement_busy.get_untracked()
                || requested_revision != order_revision.get_untracked()
                || requested_context != search_context.get_untracked()
            {
                TimeoutFuture::new(100).await;
                continue;
            }
            match loaded {
                Ok(items) => {
                    let retain_index = |index: usize| {
                        if loaded_context.get_untracked().as_ref() == Some(&requested_context)
                            && let Some(id) =
                                clips.with_untracked(|old| old.get(index).map(|item| item.id))
                            && let Some(next) = items.iter().position(|item| item.id == id)
                        {
                            next
                        } else {
                            index.min(items.len().saturating_sub(1))
                        }
                    };
                    let selected_index = retain_index(selected.get_untracked());
                    set_selection_anchor.set(retain_index(selection_anchor.get_untracked()));
                    let valid_ids = items.iter().map(|item| item.id).collect::<HashSet<_>>();
                    set_selected.set(selected_index);
                    set_selected_ids.update(|ids| ids.retain(|id| valid_ids.contains(id)));
                    if selected_ids.get_untracked().is_empty()
                        && let Some(item) = items.get(selected_index)
                    {
                        set_selected_ids.set(HashSet::from([item.id]));
                        set_selection_anchor.set(selected_index);
                    }
                    if let Some(target) = jump_target.get_untracked()
                        && let Some(index) = items.iter().position(|item| item.id == target)
                    {
                        set_selected.set(index);
                        set_selected_ids.set(HashSet::from([target]));
                        set_selection_anchor.set(index);
                        set_jump_target.set(None);
                    }
                    let stale_previews = previews
                        .with_untracked(|cached| cached.keys().any(|id| !valid_ids.contains(id)));
                    if stale_previews {
                        set_previews.update(|cached| cached.retain(|id, _| valid_ids.contains(id)));
                    }
                    preview_attempts
                        .update(|attempts| attempts.retain(|id, _| valid_ids.contains(id)));
                    for (clip_id, content_hash) in items
                        .iter()
                        .filter(|item| {
                            matches!(item.content_kind, ContentKind::Image | ContentKind::Pdf)
                        })
                        .map(|item| (item.id, item.content_hash))
                    {
                        let now = js_sys::Date::now();
                        let same_attempt = preview_attempts
                            .with_untracked(|attempts| attempts.get(&clip_id).copied())
                            .filter(|(hash, _)| *hash == content_hash);
                        if preview_loading.get_untracked().contains(&clip_id)
                            || same_attempt.is_some_and(|(_, at)| {
                                previews.get_untracked().contains_key(&clip_id)
                                    || now - at < 60_000.0
                            })
                        {
                            continue;
                        }
                        // Content-hash changes invalidate the card bitmap. A
                        // failed/encrypted PDF is retried at most once/minute,
                        // not on every background history poll.
                        if previews.with_untracked(|cached| cached.contains_key(&clip_id)) {
                            set_previews.update(|cached| {
                                cached.remove(&clip_id);
                            });
                        }
                        preview_attempts.update(|attempts| {
                            attempts.insert(clip_id, (content_hash, now));
                        });
                        set_preview_loading.update(|loading| {
                            loading.insert(clip_id);
                        });
                        spawn_local(async move {
                            let result = invoke::<PreviewResult>(
                                "get_clip_thumbnail",
                                &CommandArgs {
                                    request: PreviewRequest {
                                        clip_id: clip_id.to_string(),
                                    },
                                },
                            )
                            .await;
                            if let Ok(preview) = result
                                && clips.with_untracked(|items| {
                                    items.iter().any(|item| {
                                        item.id == clip_id && item.content_hash == content_hash
                                    })
                                })
                            {
                                set_previews.update(|items| {
                                    items.insert(clip_id, preview);
                                });
                            }
                            set_preview_loading.update(|loading| {
                                loading.remove(&clip_id);
                            });
                        });
                    }
                    if !icons_loading.get_untracked() {
                        let now = js_sys::Date::now() as u64;
                        let mut seen_sources = HashSet::new();
                        let pending = items
                            .iter()
                            .filter(|item| {
                                let key = &item.source.bundle_identifier;
                                crate::source_icon::valid_bundle_id(key)
                                    && seen_sources.insert(key.clone())
                                    && source_icons
                                        .with_untracked(|cache| cache.get(key, now).is_none())
                            })
                            .take(crate::source_icon::MAX_ICON_BATCH)
                            .map(|item| (item.id, item.source.bundle_identifier.clone()))
                            .collect::<Vec<_>>();
                        if !pending.is_empty() {
                            icons_loading.set(true);
                            spawn_local(async move {
                                let result = invoke::<Vec<SourceIconResult>>(
                                    "get_source_icons",
                                    &CommandArgs {
                                        request: ClipsRequest {
                                            clip_ids: pending
                                                .iter()
                                                .map(|(id, _)| id.to_string())
                                                .collect(),
                                        },
                                    },
                                )
                                .await;
                                let mut returned = result
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|row| {
                                        (
                                            row.bundle_identifier,
                                            row.data_url.filter(|url| {
                                                crate::source_icon::safe_icon_url(url)
                                            }),
                                        )
                                    })
                                    .collect::<HashMap<_, _>>();
                                let _ = source_icons.try_update(|cache| {
                                    for (_, key) in pending {
                                        let value = returned.remove(&key).flatten();
                                        cache.insert(key, value, js_sys::Date::now() as u64);
                                    }
                                });
                                let _ = icons_loading.try_set(false);
                            });
                        }
                    }
                    let action_focus = document()
                        .active_element()
                        .filter(|element| element.matches(".card-actions button").unwrap_or(false))
                        .and_then(|element| {
                            let card = element.closest(".clip-card").ok().flatten()?;
                            let selector = if element.matches(".locate-button").unwrap_or(false) {
                                ".locate-button"
                            } else {
                                ".stack-toggle"
                            };
                            Some((element, card.id(), selector))
                        });
                    set_clips.set(items);
                    if let Some((previous, card_id, selector)) = action_focus {
                        spawn_local(async move {
                            TimeoutFuture::new(0).await;
                            if previous.is_connected()
                                || document()
                                    .active_element()
                                    .is_some_and(|element| element.tag_name() != "BODY")
                            {
                                return;
                            }
                            // Reordering can replace the keyed card, while a
                            // remote deletion can remove it entirely. Never
                            // override a focus change the user made meanwhile.
                            let target = document()
                                .get_element_by_id(&card_id)
                                .and_then(|card| card.query_selector(selector).ok().flatten())
                                .or_else(|| document().get_element_by_id("history-results"));
                            if let Some(target) = target
                                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                            {
                                if target.matches(".card-actions button").unwrap_or(false) {
                                    focus_card_button(&target);
                                } else {
                                    let _ = target.focus();
                                }
                            }
                        });
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
            loaded_context.set(Some(requested_context.clone()));
            if let Ok(items) = invoke::<Vec<Pinboard>>("list_pinboards", &EmptyArgs {}).await
                && !placement_busy.get_untracked()
                && tab_drag.get_untracked().is_none()
                && requested_revision == order_revision.get_untracked()
            {
                set_pinboards.set(items);
            }
            if let Ok(facets) = invoke::<SearchFacets>("list_search_facets", &EmptyArgs {}).await {
                set_search_facets.set(facets);
            }
            // Keep background refresh inexpensive, but wake promptly for typing,
            // filter changes, board navigation, and history-position jumps.
            for _ in 0..10 {
                if requested_context != search_context.get_untracked() {
                    break;
                }
                TimeoutFuture::new(75).await;
            }
        }
    });

    // aria-activedescendant does not scroll the active grid cell for us.
    // Scroll only the timeline, preserving page position and keyboard focus.
    let active_card_id =
        Memo::new(move |_| clips.with(|items| items.get(selected.get()).map(|clip| clip.id)));
    Effect::new(move |_| {
        let id = active_card_id.get();
        set_timeout(
            move || {
                if let Some(id) = id
                    && let Some(card) = document().get_element_by_id(&format!("clip-card-{id}"))
                    && let Ok(Some(list)) = document().query_selector(".card-track")
                {
                    let bounds = card.get_bounding_client_rect();
                    let viewport = list.get_bounding_client_rect();
                    // Match the track's center snap points. A nearest-edge
                    // offset can snap back to the preceding card in WebKit,
                    // leaving the newly selected card partially clipped.
                    let delta = if bounds.left() < viewport.left() + 12.0
                        || bounds.right() > viewport.right() - 12.0
                    {
                        bounds.left() + bounds.width() / 2.0
                            - viewport.left()
                            - viewport.width() / 2.0
                    } else {
                        0.0
                    };
                    if delta != 0.0 {
                        list.set_scroll_left(list.scroll_left() + delta.round() as i32);
                    }
                }
            },
            std::time::Duration::ZERO,
        );
    });

    let focus_results = Callback::new(move |()| {
        if let Some(results) = results_view.get() {
            let _ = results.focus();
        }
    });
    Effect::new(move |_| {
        let open = content_editor_open.get();
        let Some(dialog) = content_editor_dialog.get() else {
            return;
        };
        if open {
            spawn_local(async move {
                // Mount the selected editor fields before native dialog focus
                // steps run. Re-check state so cancelled openings cannot race.
                TimeoutFuture::new(0).await;
                if !content_editor_open.get_untracked() || dialog.open() {
                    return;
                }
                editor_return_focus.set_value(
                    document()
                        .active_element()
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok()),
                );
                if dialog.show_modal().is_err() {
                    set_content_editor_open.set(false);
                    set_error.set(Some(t("无法打开内容编辑器，请重试。").into()));
                    return;
                }
                if let Ok(Some(field)) = dialog.query_selector("[autofocus]")
                    && let Ok(field) = field.dyn_into::<web_sys::HtmlElement>()
                {
                    let _ = field.focus();
                }
            });
        } else if dialog.open() {
            // Closing removes browser-enforced inertness before focus returns.
            dialog.close();
            let previous = editor_return_focus.get_value();
            editor_return_focus.set_value(None);
            spawn_local(async move {
                TimeoutFuture::new(0).await;
                if content_editor_open.get_untracked() {
                    return;
                }
                if let Some(previous) = previous.filter(|element| element.is_connected()) {
                    let _ = previous.focus();
                } else {
                    focus_results.run(());
                }
            });
        }
    });
    let close_preview = Callback::new(move |()| {
        set_preview_open.set(None);
        spawn_local(async move {
            // Restore timeline focus after removing the reader.
            TimeoutFuture::new(0).await;
            if !preview_visible.get_untracked() {
                focus_results.run(());
            }
        });
    });
    // The rail owns selection/query/queue. The auxiliary owns persistent settings
    // drafts and renders just the current item, never a second history list.
    let workspace_snapshot = RwSignal::new(WorkspaceSnapshot::default());
    let apply_workspace = Callback::new(move |snapshot: WorkspaceSnapshot| {
        if !auxiliary || snapshot.revision < workspace_snapshot.get_untracked().revision {
            return;
        }
        workspace_snapshot.set(snapshot.clone());
        match snapshot.content {
            WorkspaceContent::Closed => {
                set_settings_open.set(false);
                set_preview_open.set(None);
            }
            WorkspaceContent::Settings { tab } => {
                set_preview_open.set(None);
                settings_tab.set(match tab.as_str() {
                    "shortcuts" => "shortcuts",
                    "history" => "history",
                    "backup" => "backup",
                    "advanced" => "advanced",
                    _ => "general",
                });
                set_settings_open.set(true);
                refresh_shortcut.run(());
            }
            WorkspaceContent::Preview { clip } => {
                set_settings_open.set(false);
                let id = clip.id;
                let hash = clip.content_hash;
                let image = clip.content_kind == ContentKind::Image;
                set_clips.set(vec![*clip]);
                set_selected.set(0);
                set_selected_ids.set(HashSet::from([id]));
                loaded_context.set(Some(search_context.get_untracked()));
                set_preview_open.set(Some(id));
                if image {
                    set_previews.set(HashMap::new());
                    set_preview_loading.set(HashSet::from([id]));
                    spawn_local(async move {
                        let result = invoke::<PreviewResult>(
                            "get_clip_preview",
                            &CommandArgs {
                                request: PreviewRequest {
                                    clip_id: id.to_string(),
                                },
                            },
                        )
                        .await;
                        if clips.with_untracked(|items| {
                            items
                                .first()
                                .is_some_and(|item| item.id == id && item.content_hash == hash)
                        }) {
                            if let Ok(asset) = result {
                                set_previews.update(|cache| {
                                    cache.insert(id, asset);
                                });
                            }
                            set_preview_loading.set(HashSet::new());
                        }
                    });
                }
            }
        }
        if !matches!(
            workspace_snapshot.get_untracked().content,
            WorkspaceContent::Closed
        ) {
            spawn_local(async move {
                let _ = JsFuture::from(workspace_paint()).await;
                if workspace_snapshot.get_untracked().revision == snapshot.revision
                    && let Err(message) = invoke::<()>(
                        "present_workspace",
                        &WorkspaceReadyArgs {
                            revision: snapshot.revision,
                        },
                    )
                    .await
                {
                    set_error.set(Some(message));
                }
            });
        }
    });
    if auxiliary {
        drag_drop::subscribe(
            "pasters-workspace",
            Callback::new(move |value| {
                if let Ok(event) = serde_wasm_bindgen::from_value::<WorkspaceEvent>(value) {
                    apply_workspace.run(event.payload);
                }
            }),
            Callback::new(move |message| set_error.set(Some(message))),
        );
        // Reconcile after subscription and periodically while hidden: a load or
        // subscription race must not lose the user's very first open request.
        spawn_local(async move {
            loop {
                if let Ok(snapshot) =
                    invoke::<WorkspaceSnapshot>("get_workspace", &EmptyArgs {}).await
                    && snapshot.revision > workspace_snapshot.get_untracked().revision
                {
                    apply_workspace.run(snapshot);
                }
                TimeoutFuture::new(500).await;
            }
        });
        Effect::new(move |_| {
            if !expanded_visible.get()
                && !matches!(workspace_snapshot.get().content, WorkspaceContent::Closed)
            {
                spawn_local(async move {
                    if let Err(message) = invoke::<()>("dismiss_workspace", &EmptyArgs {}).await {
                        set_error.set(Some(message));
                    }
                });
            }
        });
    }
    let preview_item = Memo::new(move |previous: Option<&Option<ClipItem>>| {
        preview_open
            .get()
            .and_then(|id| clips.with(|items| items.iter().find(|item| item.id == id).cloned()))
            .or_else(|| previous.cloned().flatten())
    });
    let requested_workspace = Memo::new(move |_| {
        if settings_open.get() {
            WorkspaceContent::Settings {
                tab: settings_tab.get().into(),
            }
        } else {
            preview_open
                .get()
                .and_then(|id| clips.with(|items| items.iter().find(|item| item.id == id).cloned()))
                .map(|clip| WorkspaceContent::Preview {
                    clip: Box::new(clip),
                })
                .unwrap_or_default()
        }
    });
    let workspace_sending = RwSignal::new(false);
    if native_rail {
        Effect::new(move |_| {
            let _ = requested_workspace.get();
            if workspace_sending.get_untracked() {
                return;
            }
            workspace_sending.set(true);
            spawn_local(async move {
                loop {
                    let request = requested_workspace.get_untracked();
                    if let Err(message) = invoke::<()>(
                        "update_workspace",
                        &CommandArgs {
                            request: request.clone(),
                        },
                    )
                    .await
                    {
                        set_error.set(Some(message));
                        break;
                    }
                    if requested_workspace.get_untracked() == request {
                        break;
                    }
                }
                workspace_sending.set(false);
            });
        });
        drag_drop::subscribe(
            "pasters-workspace-key",
            Callback::new(move |value| {
                if let Ok(event) = serde_wasm_bindgen::from_value::<WorkspaceKeyEvent>(value) {
                    dispatch_workspace_key(
                        &event.payload.key,
                        event.payload.shift,
                        event.payload.meta,
                    );
                }
            }),
            Callback::new(move |message| set_error.set(Some(message))),
        );
    }
    drag_drop::subscribe(
        "pasters-preferences-changed",
        Callback::new(move |_| {
            spawn_local(async move {
                if let Ok(settings) =
                    invoke::<paste_domain::LanguageSettings>("get_language_settings", &EmptyArgs {})
                        .await
                {
                    crate::i18n::set_language(settings.effective);
                    set_desktop_preferences.update(|draft| draft.language = settings.preference);
                }
                if native_rail
                    && let Ok(saved) =
                        invoke::<DesktopPreferences>("get_desktop_preferences", &EmptyArgs {}).await
                {
                    set_desktop_preferences.set(saved);
                    set_applied_compact.set(saved.compact_mode);
                }
            });
        }),
        Callback::new(move |message| set_error.set(Some(message))),
    );
    let dismiss_search = Callback::new(move |()| {
        set_query.set(String::new());
        set_active_kind.set(None);
        set_active_source.set(None);
        set_active_device.set(None);
        set_date_days.set(None);
        set_history_offset.set(0);
        set_filter_menu_open.set(false);
        focus_results.run(());
    });
    let locate_clip = Callback::new(move |clip_id: ClipId| {
        spawn_local(async move {
            match invoke::<HistoryPositionResult>(
                "history_position",
                &CommandArgs {
                    request: ClipRequest {
                        clip_id: clip_id.to_string(),
                    },
                },
            )
            .await
            {
                Ok(result) => {
                    dismiss_search.run(());
                    set_active_pinboard.set(None);
                    set_history_offset.set(result.position.saturating_sub(3));
                    set_jump_target.set(Some(clip_id));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    });
    let undo_delete = Callback::new(move |()| {
        spawn_local(async move {
            match invoke::<Vec<ClipItem>>("undo_last_delete", &EmptyArgs {}).await {
                Ok(items) if !items.is_empty() => {
                    set_notice.set(Some(localized_format!(
                        "已恢复 {} 项内容。",
                        "Restored {} items.",
                        items.len()
                    )));
                    set_error.set(None);
                }
                Ok(_) => set_error.set(Some(t("没有可撤销的删除操作。").into())),
                Err(message) => set_error.set(Some(message)),
            }
        });
    });
    let select_all_clips = Callback::new(move |()| {
        if results_pending.get_untracked() {
            return;
        }
        set_selected.set(0);
        set_selection_anchor.set(0);
        set_selected_ids.set(clips.get_untracked().iter().map(|clip| clip.id).collect());
    });
    drag_drop::subscribe(
        "pasters-close-preview",
        Callback::new(move |_| {
            if settings_open.get_untracked() {
                set_settings_open.set(false);
                focus_results.run(());
            } else {
                close_preview.run(());
            }
        }),
        Callback::new(move |message| set_error.set(Some(message))),
    );
    drag_drop::subscribe(
        "pasters-edit-action",
        Callback::new(move |value| {
            if context_menu_open.get_untracked() {
                return;
            }
            let Ok(event) = serde_wasm_bindgen::from_value::<NativeEditEvent>(value) else {
                return;
            };
            if preview_visible.get_untracked()
                && document().active_element().is_some_and(|element| {
                    element.closest(".preview-overlay").ok().flatten().is_some()
                })
            {
                use crate::preview_edit::{Route, route};
                let doc = document();
                let text = doc
                    .query_selector(".preview-overlay .preview-text")
                    .ok()
                    .flatten();
                let document_focused = doc.active_element().is_some_and(|el| {
                    el.tag_name() == "IFRAME"
                        && el.closest(".preview-overlay").ok().flatten().is_some()
                });
                let selection = doc.get_selection().ok().flatten();
                let selected_text =
                    text.as_ref()
                        .zip(selection.as_ref())
                        .is_some_and(|(text, selection)| {
                            !selection.is_collapsed()
                                && selection
                                    .anchor_node()
                                    .is_some_and(|node| text.contains(Some(&node)))
                                && selection
                                    .focus_node()
                                    .is_some_and(|node| text.contains(Some(&node)))
                        });
                match route(
                    &event.payload,
                    document_focused,
                    text.is_some(),
                    selected_text,
                ) {
                    Route::NativeDocument => spawn_local(async move {
                        if let Err(message) = invoke::<bool>(
                            "perform_native_text_action",
                            &NativeTextActionArgs {
                                action: event.payload,
                            },
                        )
                        .await
                        {
                            set_error.set(Some(message));
                        }
                    }),
                    Route::SelectPreviewText => {
                        if let Some((text, selection)) = text.zip(selection)
                            && selection.select_all_children(&text).is_err()
                        {
                            set_error.set(Some(t("无法选择预览正文，请重新操作。").into()));
                        }
                    }
                    Route::CopyClip => {
                        // Explicitly copy the item being previewed, never a
                        // stale/multiple selection hidden behind the dialog.
                        if let Some(id) = preview_open.get_untracked() {
                            spawn_local(async move {
                                match write_clips(vec![id], false, false).await {
                                    Ok(_) => {
                                        set_error.set(None);
                                        set_notice.set(Some(t("已复制当前预览内容。").into()));
                                    }
                                    Err(message) => set_error.set(Some(message)),
                                }
                            });
                        }
                    }
                    Route::Ignore => {}
                }
                return;
            }
            if document()
                .active_element()
                .is_some_and(|element| is_text_entry(&element))
            {
                spawn_local(async move {
                    if let Err(message) = invoke::<bool>(
                        "perform_native_text_action",
                        &NativeTextActionArgs {
                            action: event.payload,
                        },
                    )
                    .await
                    {
                        set_error.set(Some(message));
                    }
                });
                return;
            }
            // Never undo history behind a modal text editor or settings form.
            if content_editor_open.get_untracked()
                || settings_open.get_untracked()
                || pinboard_creator_open.get_untracked()
                || pinboard_editor_open.get_untracked()
            {
                return;
            }
            if results_pending.get_untracked() && event.payload != "undo" {
                return;
            }
            match event.payload.as_str() {
                "undo" => undo_delete.run(()),
                "select_all" => select_all_clips.run(()),
                "copy" => {
                    let clip_ids = ordered_selection(
                        &clips.get_untracked(),
                        &selected_ids.get_untracked(),
                        selected.get_untracked(),
                    );
                    if !clip_ids.is_empty() {
                        spawn_local(async move {
                            match write_clips(clip_ids, false, false).await {
                                Ok(_) => {
                                    set_error.set(None);
                                    set_notice.set(Some(t("已复制选中内容。").into()));
                                }
                                Err(message) => set_error.set(Some(message)),
                            }
                        });
                    }
                }
                "cut" | "paste" => {
                    set_notice.set(Some(t("请先进入内容编辑器，在输入框中剪切或粘贴。").into()))
                }
                "redo" => set_notice.set(Some(t("列表当前没有可重做的操作。").into())),
                _ => {}
            }
        }),
        Callback::new(move |message| set_error.set(Some(message))),
    );

    let activate_clip = Callback::new(move |(clip_id, plain_text): (ClipId, bool)| {
        spawn_local(async move {
            match restore_clip(clip_id, plain_text).await {
                Ok(result) if result.paste_requested => set_error.set(None),
                Ok(_) => set_error.set(Some(
                    t("内容已复制；未确认当前目标，请手动粘贴或从目标应用重新唤起 CopyRail。")
                        .into(),
                )),
                Err(message) => set_error.set(Some(message)),
            }
        });
    });

    let on_keydown = move |event: ev::KeyboardEvent| {
        if auxiliary && !event.is_composing() {
            if event.key() == "Escape" {
                event.prevent_default();
                spawn_local(async move {
                    let _ = invoke::<()>("dismiss_workspace", &EmptyArgs {}).await;
                });
                return;
            }
            let key = WorkspaceKey {
                key: event.key(),
                shift: event.shift_key(),
                meta: event.meta_key(),
            };
            if matches!(event.key().as_str(), "Enter" | " ")
                && event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|element| element.closest("button, a").ok().flatten().is_some())
            {
                return;
            }
            if preview_visible.get_untracked()
                && !is_text_entry_target(&event)
                && key.valid()
                && !event.ctrl_key()
                && !event.alt_key()
            {
                event.prevent_default();
                spawn_local(async move {
                    let _ = invoke::<()>("workspace_key", &CommandArgs { request: key }).await;
                });
                return;
            }
        }
        if context_menu_open.get_untracked() {
            // AppKit owns menu navigation, Escape and Return while tracking.
            return;
        }
        if card_press.get_untracked().is_some() {
            if event.key() == "Escape" {
                card_press.set(None);
            }
            event.prevent_default();
            return;
        }
        if event.is_composing() || (event.ctrl_key() && event.alt_key()) {
            // Control+Option belongs to VoiceOver, not timeline navigation.
            return;
        }
        let current_clips = clips.get_untracked();
        let count = current_clips.len();
        let selected_clip = || {
            current_clips
                .get(selected.get_untracked())
                .map(|clip| clip.id)
        };
        let selected_clip_ids = || {
            ordered_selection(
                &current_clips,
                &selected_ids.get_untracked(),
                selected.get_untracked(),
            )
        };
        if content_editor_open.get_untracked() {
            if event.key() == "Tab"
                && !event.meta_key()
                && !event.ctrl_key()
                && !event.alt_key()
                && let Some(dialog) = content_editor_dialog.get()
                && let Ok(nodes) = dialog.query_selector_all(
                    "button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href], [tabindex]",
                )
            {
                // Native modal inertness blocks background controls, but some
                // engines send boundary Tab to browser chrome (BODY in a
                // WebView). Keep both boundaries within this editor.
                let fields = (0..nodes.length())
                    .filter_map(|index| nodes.item(index))
                    .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                    .filter(|field| {
                        let bounds = field.get_bounding_client_rect();
                        field.tab_index() >= 0 && bounds.width() > 0.0 && bounds.height() > 0.0
                    })
                    .collect::<Vec<_>>();
                let active = document()
                    .active_element()
                    .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok());
                let boundary = if event.shift_key() {
                    fields.first()
                } else {
                    fields.last()
                };
                if boundary.is_some() && active.as_ref() == boundary {
                    event.prevent_default();
                    let target = if event.shift_key() { fields.last() } else { fields.first() };
                    if let Some(target) = target {
                        let _ = target.focus();
                    }
                }
            }
            if event.key() == "Escape" {
                event.prevent_default();
                set_content_editor_open.set(false);
                set_content_editor_id.set(None);
                set_content_editor_rename_only.set(false);
                // The dialog effect restores focus after releasing inertness.
            }
            return;
        }
        if settings_open.get_untracked()
            || pinboard_creator_open.get_untracked()
            || pinboard_editor_open.get_untracked()
        {
            if event.key() == "Escape" {
                event.prevent_default();
                set_settings_open.set(false);
                set_pinboard_creator_open.set(false);
                set_pinboard_editor_open.set(false);
                focus_results.run(());
            }
            return;
        }
        if let Some(clip_id) = preview_open.get_untracked() {
            if !is_text_entry_target(&event)
                && let Some(next) = crate::card_navigation::navigation_index(
                    &event.key(),
                    selected.get_untracked(),
                    count,
                    event.meta_key() || event.ctrl_key() || event.alt_key(),
                )
            {
                event.prevent_default();
                set_selected.set(next);
                set_selection_anchor.set(next);
                set_selected_ids.set(HashSet::from([current_clips[next].id]));
                set_preview_open.set(Some(current_clips[next].id));
                return;
            }
            if matches!(event.key().as_str(), "Escape" | " ") && !is_text_entry_target(&event) {
                event.prevent_default();
                close_preview.run(());
            } else if event.key() == "Enter"
                && !is_text_entry_target(&event)
                && !event.meta_key()
                && !event.ctrl_key()
                && !event.alt_key()
                && !event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|element| element.closest("button, a").ok().flatten().is_some())
            {
                event.prevent_default();
                activate_clip.run((clip_id, event.shift_key()));
            }
            return;
        }
        if event.meta_key() && event.key().eq_ignore_ascii_case("f") {
            event.prevent_default();
            set_filter_menu_open.set(false);
            set_pin_menu_open.set(false);
            if let Some(input) = search_input.get() {
                let _ = input.focus();
            }
            return;
        }
        if filter_menu_open.get_untracked() || pin_menu_open.get_untracked() {
            if event.key() == "Escape" {
                event.prevent_default();
                set_filter_menu_open.set(false);
                set_pin_menu_open.set(false);
                if let Some(input) = search_input.get() {
                    let _ = input.focus();
                }
            }
            return;
        }
        let target = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok());
        let search_focused = target
            .as_ref()
            .is_some_and(|element| element.id() == "history-search");
        let results_focused = target.as_ref().is_some_and(|element| {
            element
                .matches(".paste-shell, .timeline, .clip-card[role='gridcell']")
                .unwrap_or(false)
        });
        if let Some(button) = target
            .as_ref()
            .and_then(|element| element.closest(".card-actions button").ok().flatten())
        {
            if let Some(action) = crate::card_navigation::action_key(
                &event.key(),
                event.shift_key(),
                event.meta_key() || event.ctrl_key() || event.alt_key(),
            ) {
                event.prevent_default();
                let buttons = button
                    .closest(".card-actions")
                    .ok()
                    .flatten()
                    .and_then(|actions| actions.query_selector_all("button:not([disabled])").ok())
                    .map(|nodes| {
                        (0..nodes.length())
                            .filter_map(|index| nodes.item(index))
                            .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let current = buttons
                    .iter()
                    .position(|element| element.is_same_node(Some(&button)))
                    .unwrap_or(0);
                if let Some(next) =
                    crate::card_navigation::action_target(action, current, buttons.len())
                {
                    let _ = buttons[next].focus();
                } else {
                    focus_results.run(());
                }
            }
            // Space/Return remain native button activation; ordinary arrows,
            // Escape, and typing here never act on another selected card.
            return;
        }
        if let Some(action) = search::key_action(
            &event.key(),
            search_focused,
            results_focused,
            search_active.get_untracked(),
            event.meta_key() || event.ctrl_key() || event.alt_key(),
            event.is_composing(),
        ) {
            event.prevent_default();
            match action {
                SearchKeyAction::Results => focus_results.run(()),
                SearchKeyAction::Search => {
                    if let Some(input) = search_input.get() {
                        let _ = input.focus();
                    }
                }
                SearchKeyAction::Dismiss => dismiss_search.run(()),
                SearchKeyAction::Type => {
                    set_query.update(|text| text.push_str(&event.key()));
                    set_history_offset.set(0);
                    if let Some(input) = search_input.get() {
                        let _ = input.focus();
                    }
                }
            }
            return;
        }
        if is_text_entry_target(&event) {
            return;
        }
        if matches!(event.key().as_str(), "Enter" | " ")
            && target
                .as_ref()
                .is_some_and(|element| element.closest("button, a").ok().flatten().is_some())
        {
            // Let a keyboard-focused toolbar/card button perform its own action.
            return;
        }
        // Focus and cancellation remain usable while a new query is loading,
        // but no key may act on a previous query's still-pending selection.
        if results_pending.get_untracked() && event.key() != "Escape" {
            event.prevent_default();
            return;
        }
        if results_focused
            && event.key() == "F2"
            && !event.meta_key()
            && !event.ctrl_key()
            && !event.alt_key()
        {
            event.prevent_default();
            if let Some(clip_id) = selected_clip()
                && let Some(card) = document().get_element_by_id(&format!("clip-card-{clip_id}"))
                && let Ok(Some(button)) =
                    card.query_selector(".card-actions button:not([disabled])")
                && let Ok(button) = button.dyn_into::<web_sys::HtmlElement>()
            {
                focus_card_button(&button);
            }
            return;
        }
        if event.key() == "ContextMenu" || (event.shift_key() && event.key() == "F10") {
            event.prevent_default();
            if let Some(clip_id) = selected_clip()
                && let Ok(Some(card)) =
                    document().query_selector(&format!("[data-drop-clip='{clip_id}']"))
                && let Some(callback) = context_menu_callback.get_value()
            {
                let rect = card.get_bounding_client_rect();
                let width = window()
                    .inner_width()
                    .ok()
                    .and_then(|value| value.as_f64())
                    .unwrap_or(1440.0);
                let height = window()
                    .inner_height()
                    .ok()
                    .and_then(|value| value.as_f64())
                    .unwrap_or(248.0);
                callback.run((
                    clip_id,
                    (rect.x() + 16.0).clamp(0.0, width),
                    (rect.y() + 16.0).clamp(0.0, height),
                ));
            }
            return;
        }
        if event.meta_key() && event.key().eq_ignore_ascii_case("g") {
            event.prevent_default();
            let ids = selected_ids.get_untracked();
            if ids.len() == 1
                && let Some(id) = ids.iter().next().copied()
            {
                locate_clip.run(id);
            }
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("n") {
            event.prevent_default();
            if event.shift_key() {
                set_pinboard_creator_open.set(true);
                set_pinboard_editor_open.set(false);
            } else {
                set_content_editor_id.set(None);
                set_content_editor_kind.set(ContentKind::Text);
                set_content_editor_title.set(String::new());
                set_content_editor_value.set(String::new());
                set_content_editor_rename_only.set(false);
                set_content_editor_open.set(true);
            }
            set_settings_open.set(false);
            set_pin_menu_open.set(false);
            set_filter_menu_open.set(false);
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("e") {
            event.prevent_default();
            let selected_values = selected_ids.get_untracked();
            if selected_values.len() == 1
                && let Some(clip) = current_clips
                    .iter()
                    .find(|clip| selected_values.contains(&clip.id))
            {
                if needs_rich_text_editor(clip) {
                    let id = clip.id;
                    spawn_local(async move {
                        match invoke::<()>(
                            "open_rich_text_editor",
                            &CommandArgs {
                                request: ClipRequest {
                                    clip_id: id.to_string(),
                                },
                            },
                        )
                        .await
                        {
                            Ok(()) => set_error.set(None),
                            Err(message) => set_error.set(Some(message)),
                        }
                    });
                } else if is_textually_editable(clip.content_kind) {
                    set_content_editor_id.set(Some(clip.id));
                    set_content_editor_kind.set(clip.content_kind);
                    set_content_editor_title.set(clip.title.clone());
                    set_content_editor_value.set(clip.searchable_text.clone());
                    set_content_editor_rename_only.set(false);
                    set_content_editor_open.set(true);
                    set_error.set(None);
                } else if clip.content_kind == ContentKind::Image {
                    set_preview_open.set(Some(clip.id));
                    set_error.set(None);
                } else {
                    set_error.set(Some(t("当前类型暂不支持内容编辑。").into()));
                }
            }
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("r") {
            event.prevent_default();
            let selected_values = selected_ids.get_untracked();
            if selected_values.len() == 1
                && let Some(clip) = current_clips
                    .iter()
                    .find(|clip| selected_values.contains(&clip.id))
            {
                set_content_editor_id.set(Some(clip.id));
                set_content_editor_kind.set(clip.content_kind);
                set_content_editor_title.set(clip.title.clone());
                set_content_editor_value.set(clip.searchable_text.clone());
                set_content_editor_rename_only.set(true);
                set_content_editor_open.set(true);
                set_error.set(None);
            }
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("c") {
            event.prevent_default();
            let clip_ids = selected_clip_ids();
            if !clip_ids.is_empty() {
                spawn_local(async move {
                    match write_clips(clip_ids, event.shift_key(), false).await {
                        Ok(_) => {
                            set_notice.set(Some(t("已复制选中内容。").into()));
                            set_error.set(None);
                        }
                        Err(message) => set_error.set(Some(message)),
                    }
                });
            }
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("o") {
            event.prevent_default();
            let selected_values = selected_ids.get_untracked();
            if selected_values.len() == 1
                && let Some(clip) = current_clips
                    .iter()
                    .find(|clip| selected_values.contains(&clip.id))
            {
                if clip.content_kind == ContentKind::Link {
                    let clip_id = clip.id;
                    spawn_local(async move {
                        match open_link_preview(clip_id).await {
                            Ok(()) => {
                                set_notice
                                    .set(Some(t("已在 CopyRail 内置浏览器中打开链接。").into()));
                                set_error.set(None);
                            }
                            Err(message) => set_error.set(Some(message)),
                        }
                    });
                } else {
                    set_error.set(Some(t("当前仅支持在内置浏览器中打开链接。").into()));
                }
            }
            return;
        }

        if event.key() == " " {
            event.prevent_default();
            if preview_open.get_untracked().is_some() {
                set_preview_open.set(None);
            } else if let Some(clip_id) = selected_clip() {
                set_preview_open.set(Some(clip_id));
            }
            return;
        }

        if event.meta_key() && event.key() == "Enter" {
            event.prevent_default();
            let clip_ids = selected_clip_ids();
            set_stack.update(|items| {
                for clip_id in clip_ids {
                    toggle_stack_item(items, clip_id);
                }
            });
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("a") && count > 0 {
            event.prevent_default();
            select_all_clips.run(());
            return;
        }

        if event.meta_key() && matches!(event.key().as_str(), "ArrowUp" | "ArrowDown") && count > 0
        {
            event.prevent_default();
            let index = if event.key() == "ArrowUp" {
                0
            } else {
                count - 1
            };
            set_selected.set(index);
            set_selection_anchor.set(index);
            set_selected_ids.set(HashSet::from([current_clips[index].id]));
            return;
        }

        if results_focused
            && let Some(next) = crate::card_navigation::navigation_index(
                &event.key(),
                selected.get_untracked(),
                count,
                event.meta_key() || event.ctrl_key() || event.alt_key(),
            )
        {
            event.prevent_default();
            focus_results.run(());
            set_selected.set(next);
            if event.shift_key() {
                set_selected_ids.set(selection_range(
                    &current_clips,
                    selection_anchor.get_untracked(),
                    next,
                ));
            } else {
                set_selection_anchor.set(next);
                set_selected_ids.set(HashSet::from([current_clips[next].id]));
            }
            return;
        }

        if event.meta_key() && event.key().eq_ignore_ascii_case("z") {
            event.prevent_default();
            if !event.shift_key() {
                undo_delete.run(());
            }
            return;
        }

        if event.meta_key()
            && let Some(index) = quick_paste_index(&event.key())
            && let Some(clip_id) = clips.get_untracked().get(index).map(|clip| clip.id)
        {
            event.prevent_default();
            spawn_local(async move {
                match restore_clip(clip_id, false).await {
                    Ok(result) if result.paste_requested => set_error.set(None),
                    Ok(_) => set_error.set(Some(
                        t("内容已复制；未确认当前目标，请手动粘贴或从目标应用重新唤起 CopyRail。")
                            .into(),
                    )),
                    Err(message) => set_error.set(Some(message)),
                }
            });
            return;
        }

        match event.key().as_str() {
            "Enter" => {
                event.prevent_default();
                let queued = stack.get_untracked().first().copied();
                let clip_ids = queued.map_or_else(selected_clip_ids, |clip_id| vec![clip_id]);
                if !clip_ids.is_empty() {
                    let plain_text = event.shift_key();
                    spawn_local(async move {
                        match restore_clips(clip_ids, plain_text).await {
                            Ok(result) if result.paste_requested => {
                                if let Some(clip_id) = queued {
                                    set_stack.update(|items| {
                                        if items.first() == Some(&clip_id) {
                                            items.remove(0);
                                        }
                                    });
                                }
                                set_error.set(None);
                            }
                            Ok(_) => set_error.set(Some(
                                t("内容已复制；未确认当前目标，请手动粘贴或从目标应用重新唤起 CopyRail。")
                                    .into(),
                            )),
                            Err(message) => set_error.set(Some(message)),
                        }
                    });
                }
            }
            "Backspace" | "Delete" => {
                event.prevent_default();
                let clip_ids = selected_clip_ids();
                if !clip_ids.is_empty() {
                    let removed = clip_ids.iter().copied().collect::<HashSet<_>>();
                    spawn_local(async move {
                        match invoke::<()>(
                            "delete_clips",
                            &CommandArgs {
                                request: ClipsRequest {
                                    clip_ids: clip_ids
                                        .into_iter()
                                        .map(|clip_id| clip_id.to_string())
                                        .collect(),
                                },
                            },
                        )
                        .await
                        {
                            Ok(()) => {
                                set_clips.update(|items| {
                                    items.retain(|item| !removed.contains(&item.id));
                                });
                                set_stack.update(|items| {
                                    items.retain(|item| !removed.contains(item));
                                });
                                set_selected_ids.set(HashSet::new());
                                set_selected.update(|index| {
                                    *index =
                                        (*index).min(clips.get_untracked().len().saturating_sub(1));
                                });
                                set_error.set(None);
                            }
                            Err(message) => set_error.set(Some(message)),
                        }
                    });
                }
            }
            "Escape" if preview_open.get_untracked().is_some() => set_preview_open.set(None),
            "Escape" => spawn_local(async {
                let _ = invoke::<()>("hide_window", &EmptyArgs {}).await;
            }),
            _ => {}
        }
    };

    let pause = move |_| {
        if capture_request_busy.get_untracked().is_some()
            || status
                .get_untracked()
                .is_some_and(|value| value.isolated || value.control_pending.is_some())
        {
            return;
        }
        capture_request_busy.set(Some("pause"));
        spawn_local(async move {
            let result = invoke::<CaptureStatus>(
                "pause_capture",
                &CommandArgs {
                    request: PauseRequest { minutes: Some(15) },
                },
            )
            .await;
            match result {
                Ok(current) => apply_capture_status.run(current),
                Err(message) => set_error.set(Some(message)),
            }
            capture_request_busy.set(None);
        });
    };

    let resume = move |_| {
        if capture_request_busy.get_untracked().is_some()
            || status
                .get_untracked()
                .is_some_and(|value| value.isolated || value.control_pending.is_some())
        {
            return;
        }
        capture_request_busy.set(Some("resume"));
        spawn_local(async move {
            match invoke::<CaptureStatus>("resume_capture", &EmptyArgs {}).await {
                Ok(current) => apply_capture_status.run(current),
                Err(message) => set_error.set(Some(message)),
            }
            capture_request_busy.set(None);
        });
    };

    let toggle_stack = Callback::new(move |clip_id: ClipId| {
        set_stack.update(|items| toggle_stack_item(items, clip_id));
    });

    let drag_selection = Callback::new(move |clip_id: ClipId| {
        if placement_busy.get_untracked() || native_dragging.get_untracked() {
            return;
        }
        native_dragging.set(true);
        native_feedback_active.set(false);
        drop_target.set(None);
        // A previous successful move is not the outcome of this new gesture.
        // Clear it at the accepted start (not on a possibly stale end event),
        // so cancellation or an invalid drop cannot leave false success text.
        set_notice.set(None);
        let current_clips = clips.get_untracked();
        let current_selection = selected_ids.get_untracked();
        let clip_ids = if current_selection.contains(&clip_id) {
            ordered_selection(&current_clips, &current_selection, selected.get_untracked())
        } else {
            if let Some(index) = current_clips.iter().position(|clip| clip.id == clip_id) {
                set_selected.set(index);
                set_selection_anchor.set(index);
            }
            set_selected_ids.set(HashSet::from([clip_id]));
            vec![clip_id]
        };
        let feedback = drag_drop::feedback_layout();
        spawn_local(async move {
            match invoke::<DragExportResult>(
                "start_clip_drag",
                &CommandArgs {
                    request: StartDragRequest {
                        clip_ids: clip_ids
                            .into_iter()
                            .map(|clip_id| clip_id.to_string())
                            .collect(),
                        feedback,
                    },
                },
            )
            .await
            {
                Ok(result) if result.item_count > 0 => set_error.set(None),
                Ok(_) => {
                    native_dragging.set(false);
                    set_error.set(Some(t("没有可拖出的内容。").into()));
                }
                Err(message) => {
                    native_dragging.set(false);
                    set_error.set(Some(message));
                }
            }
        });
    });

    drag_drop::subscribe(
        "pasters-drag-ended",
        Callback::new(move |value| {
            let Some(event) = drag_drop::decode_end(value) else {
                return;
            };
            if drag_lifecycle.with_value(|state| {
                state
                    .borrow_mut()
                    .finish(&event.session_id, event.cancelled)
            }) {
                native_dragging.set(false);
                native_feedback_active.set(false);
                drop_target.set(None);
            }
            drag_layouts.with_value(|state| state.borrow_mut().finish(&event.session_id));
        }),
        Callback::new(move |message| set_error.set(Some(message))),
    );
    drag_drop::subscribe(
        "pasters-internal-drag",
        Callback::new(move |value| {
            let Some(event) = drag_drop::decode(value) else {
                return;
            };
            if event.session_id.is_empty() {
                return;
            }
            let Some(clear_current) = drag_lifecycle
                .with_value(|state| state.borrow_mut().route(&event.session_id, &event.phase))
            else {
                return;
            };
            if clear_current {
                native_feedback_active.set(event.placement_feedback.is_some());
            }
            if event.phase == "leave" {
                drop_target.set(None);
                return;
            }
            if placement_busy.get_untracked() {
                return;
            }
            if matches!(event.phase.as_str(), "enter" | "over") {
                drag_drop::scroll_at_edge(event.x, event.y);
                let next = drag_layouts.with_value(|state| {
                    state
                        .borrow_mut()
                        .observe(&event.session_id, drag_drop::feedback_layout())
                });
                if let Some(request) = next {
                    spawn_local(async move {
                        let mut next = Some(request);
                        while let Some(request) = next {
                            // False is normal if the source finished before a
                            // queued refresh. Never revive feedback on an ack.
                            let _ = invoke::<bool>(
                                "update_clip_drag_feedback",
                                &CommandArgs { request },
                            )
                            .await;
                            next = drag_layouts
                                .try_with_value(|state| state.borrow_mut().complete())
                                .flatten();
                        }
                    });
                }
            }
            let target = drag_drop::hit_test(
                event.x,
                event.y,
                active_pinboard.get_untracked(),
                !search_active.get_untracked(),
                &event.clip_ids,
            );
            let target = target
                .filter(|target| !target.anchor.is_some_and(|id| event.clip_ids.contains(&id)));
            let feedback_matches = if let Some(feedback) = event.placement_feedback.as_ref() {
                feedback.target == target
            } else {
                event.tab_feedback.as_ref().is_none_or(|feedback| {
                    feedback.target == drag_drop::tab_at_point(event.x, event.y)
                })
            };
            if clear_current {
                drop_target.set(target.filter(|_| feedback_matches));
            }
            if event.phase != "drop" {
                return;
            }
            drag_layouts.with_value(|state| state.borrow_mut().finish(&event.session_id));
            if clear_current {
                native_dragging.set(false);
                native_feedback_active.set(false);
                drop_target.set(None);
            }
            if !feedback_matches {
                set_error.set(Some(
                    t("分类位置已变化或排序落点已失效，本次未移动；请重新拖到目标位置。").into(),
                ));
                return;
            }
            let Some(target) = target else {
                return;
            };
            placement_busy.set(true);
            order_revision.update(|value| *value += 1);
            let count = event.clip_ids.len();
            let request = PlacePinboardClipsRequest {
                pinboard_id: target.pinboard_id.to_string(),
                clip_ids: event.clip_ids.iter().map(ToString::to_string).collect(),
                anchor: target.anchor.map(|id| id.to_string()),
                after: target.after,
            };
            spawn_local(async move {
                match invoke::<bool>("place_pinboard_clips", &CommandArgs { request }).await {
                    Ok(true) => {
                        set_notice.set(Some(localized_format!(
                            "已移动 {count} 项，Pinboard 顺序已保存。",
                            "Moved {count} items and saved the pinboard order."
                        )));
                        set_error.set(None);
                    }
                    Ok(false) => set_notice.set(Some(t("项目已在这个位置。").into())),
                    Err(message) => set_error.set(Some(message)),
                }
                placement_busy.set(false);
            });
        }),
        Callback::new(move |message| set_error.set(Some(message))),
    );

    let drop_pinboard = Callback::new(move |(target, after): (PinboardId, bool)| {
        let dragged = tab_drag.get_untracked();
        tab_drag.set(None);
        tab_hover.set(None);
        let Some(dragged) = dragged else {
            return;
        };
        if dragged == target || placement_busy.get_untracked() {
            return;
        }
        let mut ordered = pinboards.get_untracked();
        let Some(index) = ordered.iter().position(|board| board.id == dragged) else {
            return;
        };
        let board = ordered.remove(index);
        let Some(target) = ordered.iter().position(|board| board.id == target) else {
            return;
        };
        ordered.insert(target + usize::from(after), board);
        if ordered
            .iter()
            .map(|board| board.id)
            .eq(pinboards.get_untracked().iter().map(|board| board.id))
        {
            return;
        }
        let request = ReorderPinboardsRequest {
            pinboard_ids: ordered.iter().map(|board| board.id.to_string()).collect(),
        };
        placement_busy.set(true);
        order_revision.update(|value| *value += 1);
        spawn_local(async move {
            match invoke::<Vec<Pinboard>>("reorder_pinboards", &CommandArgs { request }).await {
                Ok(saved) => {
                    set_pinboards.set(saved);
                    set_notice.set(Some(t("Pinboard 顺序已保存。").into()));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
            placement_busy.set(false);
        });
    });

    let export_backup = move |_| {
        spawn_local(async move {
            match invoke::<Option<BackupActionResult>>("export_backup", &EmptyArgs {}).await {
                Ok(Some(result)) => {
                    set_notice.set(Some(localized_format!(
                        "备份已保存到 {}",
                        "Backup saved to {}",
                        result.path
                    )));
                    set_error.set(None);
                }
                Ok(None) => {}
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let restore_backup = move |_| {
        spawn_local(async move {
            match invoke::<Option<BackupActionResult>>("restore_backup", &EmptyArgs {}).await {
                Ok(Some(result)) => {
                    set_stack.set(Vec::new());
                    set_selected.set(0);
                    set_selected_ids.set(HashSet::new());
                    set_selection_anchor.set(0);
                    set_previews.set(HashMap::new());
                    set_preview_loading.set(HashSet::new());
                    set_preview_open.set(None);
                    if let Ok(preferences) =
                        invoke::<CapturePreferences>("get_capture_preferences", &EmptyArgs {}).await
                    {
                        set_retention_days.set(optional_number(preferences.retention.max_age_days));
                        set_retention_items
                            .set(optional_number(preferences.retention.max_unpinned_items));
                        set_excluded_apps.set(preferences.excluded_bundle_ids.join("\n"));
                    }
                    if let Ok(preferences) =
                        invoke::<DesktopPreferences>("get_desktop_preferences", &EmptyArgs {}).await
                    {
                        set_desktop_preferences.set(preferences);
                        set_applied_compact.set(preferences.compact_mode);
                        if let Ok(settings) = invoke::<paste_domain::LanguageSettings>(
                            "get_language_settings",
                            &EmptyArgs {},
                        )
                        .await
                        {
                            crate::i18n::set_language(settings.effective);
                        }
                    }
                    set_settings_open.set(false);
                    set_notice.set(Some(localized_format!(
                        "已从 {} 恢复本地数据",
                        "Local data restored from {}",
                        result.path
                    )));
                    set_error.set(None);
                }
                Ok(None) => {}
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let request_accessibility = move |_| {
        spawn_local(async move {
            match invoke::<PermissionStatus>("request_accessibility_permission", &EmptyArgs {})
                .await
            {
                Ok(current) => {
                    let trusted = current.accessibility_trusted;
                    set_permission_status.set(Some(current));
                    set_notice.set(Some(if trusted {
                        t("直接粘贴权限已启用。").into()
                    } else {
                        t("已请求 macOS 授权；开启 CopyRail 后请点“重新检测”。").into()
                    }));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let refresh_accessibility = move |_| {
        spawn_local(async move {
            match invoke::<PermissionStatus>("get_permission_status", &EmptyArgs {}).await {
                Ok(current) => {
                    let trusted = current.accessibility_trusted;
                    set_permission_status.set(Some(current));
                    set_notice.set(Some(if trusted {
                        t("已确认直接粘贴权限。").into()
                    } else {
                        t("尚未授予辅助功能权限；复制仍可正常使用。").into()
                    }));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let toggle_cloud_sync = move |event| {
        let enabled = event_target_checked(&event);
        spawn_local(async move {
            match invoke::<SyncStatus>(
                "set_cloud_sync_enabled",
                &CommandArgs {
                    request: CloudSyncEnabledRequest { enabled },
                },
            )
            .await
            {
                Ok(current) => {
                    let notice_message = if enabled && current.cloud_transport_configured {
                        t("iCloud 同步已启用，正在后台检查账户并同步；失败时本地队列会完整保留。")
                    } else if enabled {
                        t(
                            "已保存 iCloud 同步选择；当前构建不访问 CloudKit，本地待发送队列会完整保留。",
                        )
                    } else {
                        t("iCloud 同步已关闭；不会初始化 CloudKit。")
                    };
                    set_sync_status.set(Some(current));
                    set_notice.set(Some(notice_message.into()));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let resolve_sync_conflict =
        Callback::new(move |(conflict_id, resolution): (String, String)| {
            if resolving_conflicts.get_untracked().contains(&conflict_id) {
                return;
            }
            set_resolving_conflicts.update(|ids| {
                ids.insert(conflict_id.clone());
            });
            spawn_local(async move {
                match invoke::<Vec<SyncConflictView>>(
                    "resolve_sync_conflict",
                    &CommandArgs {
                        request: ResolveSyncConflictRequest {
                            conflict_id: conflict_id.clone(),
                            resolution: resolution.clone(),
                        },
                    },
                )
                .await
                {
                    Ok(conflicts) => {
                        set_sync_conflicts.set(conflicts);
                        if let Ok(current) =
                            invoke::<SyncStatus>("get_sync_status", &EmptyArgs {}).await
                        {
                            set_sync_status.set(Some(current));
                        }
                        set_notice.set(Some(if resolution == "keep_local" {
                            t("已保留此 Mac 的最新版本，最终选择已加入同步队列。").into()
                        } else {
                            t("已采用另一台设备的版本，最终选择已加入同步队列。").into()
                        }));
                        set_error.set(None);
                    }
                    Err(message) => set_error.set(Some(message)),
                }
                set_resolving_conflicts.update(|ids| {
                    ids.remove(&conflict_id);
                });
            });
        });

    let resolve_shared_conflict = Callback::new(
        move |(conflict_id, current_operation_id, resolution): (String, String, String)| {
            if resolving_shared_conflicts
                .get_untracked()
                .contains(&conflict_id)
            {
                return;
            }
            set_resolving_shared_conflicts.update(|ids| {
                ids.insert(conflict_id.clone());
            });
            set_notice.set(None);
            spawn_local(async move {
                match invoke::<Vec<SharedConflictView>>(
                    "resolve_shared_conflict",
                    &CommandArgs {
                        request: ResolveSharedConflictRequest {
                            conflict_id: conflict_id.clone(),
                            current_operation_id,
                            resolution,
                        },
                    },
                )
                .await
                {
                    Ok(conflicts) => {
                        set_shared_conflicts.set(conflicts);
                        set_notice
                            .set(Some(t("选择已保存并加入共享待发送队列；尚未上传。").into()));
                        set_error.set(None);
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        if let Ok(conflicts) = invoke::<Vec<SharedConflictView>>(
                            "list_shared_conflicts",
                            &EmptyArgs {},
                        )
                        .await
                        {
                            set_shared_conflicts.set(conflicts);
                        }
                    }
                }
                if let Ok(current) = invoke::<SyncStatus>("get_sync_status", &EmptyArgs {}).await {
                    set_sync_status.set(Some(current));
                }
                set_resolving_shared_conflicts.update(|ids| {
                    ids.remove(&conflict_id);
                });
            });
        },
    );

    let toggle_mcp = move |event| {
        let enabled = event_target_checked(&event);
        spawn_local(async move {
            match invoke::<McpAccessStatus>(
                "set_mcp_enabled",
                &CommandArgs {
                    request: McpEnabledRequest { enabled },
                },
            )
            .await
            {
                Ok(current) => {
                    set_mcp_status.set(Some(current));
                    set_notice.set(Some(if enabled {
                        t("MCP 本地访问已启用。").into()
                    } else {
                        t("MCP 本地访问已停用，现有连接立即失效。").into()
                    }));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let create_mcp_connection = move |_| {
        let display_name = mcp_client_name.get_untracked().trim().to_owned();
        if display_name.is_empty() {
            set_error.set(Some(t("请填写要连接的客户端名称。").into()));
            return;
        }
        spawn_local(async move {
            match invoke::<McpConnectionResult>(
                "create_mcp_connection",
                &CommandArgs {
                    request: CreateMcpConnectionRequest { display_name },
                },
            )
            .await
            {
                Ok(result) => {
                    set_mcp_status.set(Some(result.status));
                    set_mcp_connection_config.set(Some(result.configuration));
                    set_mcp_client_name.set(String::new());
                    set_notice.set(Some(
                        t("连接已创建并启用；配置只显示这一次，请立即保存到目标客户端。").into(),
                    ));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let revoke_mcp_connection = Callback::new(move |client_id: String| {
        spawn_local(async move {
            match invoke::<McpAccessStatus>(
                "revoke_mcp_connection",
                &CommandArgs {
                    request: RevokeMcpConnectionRequest { client_id },
                },
            )
            .await
            {
                Ok(current) => {
                    set_mcp_status.set(Some(current));
                    set_notice.set(Some(
                        t("该 MCP 客户端已撤销，正在运行的连接也会失效。").into(),
                    ));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    });

    let save_settings = move |_| {
        if language_saving.get_untracked() || settings_saving.get_untracked() {
            return;
        }
        let capture_preferences = CapturePreferences {
            retention: RetentionPolicy {
                max_age_days: parse_optional_positive(&retention_days.get_untracked()),
                max_unpinned_items: parse_optional_positive(&retention_items.get_untracked()),
            },
            excluded_bundle_ids: excluded_apps
                .get_untracked()
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        };
        let desktop_request = desktop_preferences.get_untracked();
        settings_saving.set(true);
        spawn_local(async move {
            match invoke::<CapturePreferences>(
                "update_capture_preferences",
                &CommandArgs {
                    request: capture_preferences,
                },
            )
            .await
            {
                Ok(saved_capture) => {
                    match invoke::<DesktopPreferences>(
                        "update_desktop_preferences",
                        &CommandArgs {
                            request: desktop_request,
                        },
                    )
                    .await
                    {
                        Ok(saved_desktop) => {
                            set_retention_days
                                .set(optional_number(saved_capture.retention.max_age_days));
                            set_retention_items
                                .set(optional_number(saved_capture.retention.max_unpinned_items));
                            set_excluded_apps.set(saved_capture.excluded_bundle_ids.join("\n"));
                            set_desktop_preferences.set(saved_desktop);
                            set_applied_compact.set(saved_desktop.compact_mode);
                            if let Ok(settings) = invoke::<paste_domain::LanguageSettings>(
                                "get_language_settings",
                                &EmptyArgs {},
                            )
                            .await
                            {
                                crate::i18n::set_language(settings.effective);
                            }
                            set_settings_open.set(false);
                            set_error.set(None);
                        }
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
            settings_saving.set(false);
        });
    };

    let create_pinboard = move |_| {
        let request = PinboardRequest {
            name: new_pinboard_name.get_untracked(),
            color: new_pinboard_color.get_untracked(),
        };
        spawn_local(async move {
            match invoke::<Pinboard>("create_pinboard", &CommandArgs { request }).await {
                Ok(pinboard) => {
                    let pinboard_id = pinboard.id;
                    set_pinboards.update(|items| items.push(pinboard));
                    set_active_pinboard.set(Some(pinboard_id));
                    set_new_pinboard_name.set(String::new());
                    set_pinboard_creator_open.set(false);
                    set_selected.set(0);
                    set_selected_ids.set(HashSet::new());
                    set_selection_anchor.set(0);
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let save_pinboard = move |_| {
        let Some(pinboard_id) = active_pinboard.get_untracked() else {
            return;
        };
        let request = UpdatePinboardRequest {
            pinboard_id: pinboard_id.to_string(),
            name: edit_pinboard_name.get_untracked(),
            color: edit_pinboard_color.get_untracked(),
        };
        spawn_local(async move {
            match invoke::<Pinboard>("update_pinboard", &CommandArgs { request }).await {
                Ok(updated) => {
                    set_pinboards.update(|items| {
                        if let Some(item) = items.iter_mut().find(|item| item.id == updated.id) {
                            *item = updated;
                        }
                    });
                    set_pinboard_editor_open.set(false);
                    set_notice.set(Some(t("Pinboard 已更新。").into()));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let move_pinboard = Callback::new(move |direction: i8| {
        let Some(pinboard_id) = active_pinboard.get_untracked() else {
            return;
        };
        let mut ordered = pinboards.get_untracked();
        let Some(index) = ordered.iter().position(|item| item.id == pinboard_id) else {
            return;
        };
        let target = if direction < 0 {
            index.checked_sub(1)
        } else {
            let next = index + 1;
            (next < ordered.len()).then_some(next)
        };
        let Some(target) = target else {
            return;
        };
        ordered.swap(index, target);
        let request = ReorderPinboardsRequest {
            pinboard_ids: ordered.iter().map(|item| item.id.to_string()).collect(),
        };
        spawn_local(async move {
            match invoke::<Vec<Pinboard>>("reorder_pinboards", &CommandArgs { request }).await {
                Ok(saved) => {
                    set_pinboards.set(saved);
                    set_notice.set(Some(t("Pinboard 顺序已保存。").into()));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    });

    let move_pinboard_item = Callback::new(move |direction: i8| {
        let Some(pinboard_id) = active_pinboard.get_untracked() else {
            return;
        };
        let selected_values = selected_ids.get_untracked();
        let Some(clip_id) = selected_values.iter().copied().next() else {
            return;
        };
        if selected_values.len() != 1 {
            return;
        }
        let request = MovePinboardItemRequest {
            pinboard_id: pinboard_id.to_string(),
            clip_id: clip_id.to_string(),
            direction,
        };
        spawn_local(async move {
            match invoke::<bool>("move_pinboard_item", &CommandArgs { request }).await {
                Ok(true) => {
                    let mut items = clips.get_untracked();
                    if let Some(index) = items.iter().position(|item| item.id == clip_id) {
                        let target = if direction < 0 {
                            index.checked_sub(1)
                        } else {
                            let next = index + 1;
                            (next < items.len()).then_some(next)
                        };
                        if let Some(target) = target {
                            items.swap(index, target);
                            set_clips.set(items);
                            set_selected.set(target);
                            set_selection_anchor.set(target);
                        }
                    }
                    set_notice.set(Some(t("Pinboard 项目顺序已保存。").into()));
                    set_error.set(None);
                }
                Ok(false) => {}
                Err(message) => set_error.set(Some(message)),
            }
        });
    });

    let delete_pinboard = move |_| {
        let Some(pinboard_id) = active_pinboard.get_untracked() else {
            return;
        };
        let request = DeletePinboardRequest {
            pinboard_id: pinboard_id.to_string(),
        };
        spawn_local(async move {
            match invoke::<bool>("delete_pinboard", &CommandArgs { request }).await {
                Ok(true) => {
                    set_pinboards.update(|items| items.retain(|item| item.id != pinboard_id));
                    set_active_pinboard.set(None);
                    set_pinboard_editor_open.set(false);
                    set_selected.set(0);
                    set_selected_ids.set(HashSet::new());
                    set_selection_anchor.set(0);
                    set_notice.set(Some(t("Pinboard 已删除，剪贴板历史保持不变。").into()));
                    set_error.set(None);
                }
                Ok(false) => {}
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let open_new_item = move |_| {
        set_content_editor_id.set(None);
        set_content_editor_kind.set(ContentKind::Text);
        set_content_editor_title.set(String::new());
        set_content_editor_value.set(String::new());
        set_content_editor_rename_only.set(false);
        set_content_editor_open.set(true);
        set_settings_open.set(false);
        set_pinboard_creator_open.set(false);
        set_pinboard_editor_open.set(false);
        set_pin_menu_open.set(false);
        set_filter_menu_open.set(false);
    };

    let open_content_editor = move |_| {
        let selected_values = selected_ids.get_untracked();
        if selected_values.len() != 1 {
            return;
        }
        let Some(clip) = clips
            .get_untracked()
            .into_iter()
            .find(|clip| selected_values.contains(&clip.id))
        else {
            return;
        };
        if clip.content_kind == ContentKind::Image {
            set_preview_open.set(Some(clip.id));
            set_error.set(None);
            return;
        }
        if needs_rich_text_editor(&clip) {
            let id = clip.id;
            spawn_local(async move {
                match invoke::<()>(
                    "open_rich_text_editor",
                    &CommandArgs {
                        request: ClipRequest {
                            clip_id: id.to_string(),
                        },
                    },
                )
                .await
                {
                    Ok(()) => set_error.set(None),
                    Err(message) => set_error.set(Some(message)),
                }
            });
            return;
        }
        if !is_textually_editable(clip.content_kind) {
            set_error.set(Some(t("当前类型暂不支持内容编辑。").into()));
            return;
        }
        set_content_editor_id.set(Some(clip.id));
        set_content_editor_kind.set(clip.content_kind);
        set_content_editor_title.set(clip.title);
        set_content_editor_value.set(clip.searchable_text);
        set_content_editor_rename_only.set(false);
        set_content_editor_open.set(true);
        set_settings_open.set(false);
        set_pinboard_creator_open.set(false);
        set_pinboard_editor_open.set(false);
        set_pin_menu_open.set(false);
        set_filter_menu_open.set(false);
        set_error.set(None);
    };

    let open_rename_editor = move |_| {
        let selected_values = selected_ids.get_untracked();
        if selected_values.len() != 1 {
            return;
        }
        let Some(clip) = clips
            .get_untracked()
            .into_iter()
            .find(|clip| selected_values.contains(&clip.id))
        else {
            return;
        };
        set_content_editor_id.set(Some(clip.id));
        set_content_editor_kind.set(clip.content_kind);
        set_content_editor_title.set(clip.title);
        set_content_editor_value.set(clip.searchable_text);
        set_content_editor_rename_only.set(true);
        set_content_editor_open.set(true);
        set_settings_open.set(false);
        set_pinboard_creator_open.set(false);
        set_pinboard_editor_open.set(false);
        set_pin_menu_open.set(false);
        set_filter_menu_open.set(false);
        set_error.set(None);
    };

    let show_context_menu = Callback::new(move |(clip_id, x, y): (ClipId, f64, f64)| {
        if context_menu_open.get_untracked()
            || native_dragging.get_untracked()
            || placement_busy.get_untracked()
            || results_pending.get_untracked()
            || content_editor_open.get_untracked()
            || settings_open.get_untracked()
        {
            return;
        }
        let current = clips.get_untracked();
        let Some(index) = current.iter().position(|clip| clip.id == clip_id) else {
            return;
        };
        // Right-click inside a selection preserves it. Outside selects only the
        // clicked card. The menu action always uses this exact ordered snapshot.
        let Some(ids) = crate::context_selection::resolve(
            &current.iter().map(|clip| clip.id).collect::<Vec<_>>(),
            &selected_ids.get_untracked(),
            clip_id,
        ) else {
            return;
        };
        set_selected.set(index);
        set_selection_anchor.set(index);
        set_selected_ids.set(ids.iter().copied().collect());
        card_press.set(None);
        set_pin_menu_open.set(false);
        set_filter_menu_open.set(false);
        focus_results.run(());
        context_menu_open.set(true);
        let requested_context = search_context.get_untracked();
        spawn_local(async move {
            let outcome: Result<(), String> = async {
                let choice = invoke::<Option<ClipContextMenuChoice>>(
                    "show_clip_context_menu",
                    &CommandArgs {
                        request: ClipContextMenuRequest {
                            clip_ids: ids.iter().map(ToString::to_string).collect(),
                            x,
                            y,
                        },
                    },
                )
                .await?;
                let Some(choice) = choice else {
                    return Ok(());
                };
                if requested_context != search_context.get_untracked() {
                    return Err(t("菜单打开期间列表已变化，请重新选择内容。").into());
                }
                match choice.choice {
                    ContextAction::Copy
                    | ContextAction::CopyPlain
                    | ContextAction::Paste
                    | ContextAction::PastePlain => {
                        let paste = matches!(
                            choice.choice,
                            ContextAction::Paste | ContextAction::PastePlain
                        );
                        let plain = matches!(
                            choice.choice,
                            ContextAction::CopyPlain | ContextAction::PastePlain
                        );
                        let result = write_clips(ids.clone(), plain, paste).await?;
                        if paste && !result.paste_requested {
                            set_notice.set(Some(
                                t("内容已复制；未确认当前目标，请手动粘贴或从目标应用重新唤起 CopyRail。").into(),
                            ));
                        } else {
                            set_notice.set(Some(
                                if paste {
                                    t("已向目标应用发送粘贴请求。")
                                } else {
                                    t("已复制选中内容。")
                                }
                                .into(),
                            ));
                        }
                    }
                    ContextAction::Preview => set_preview_open.set(Some(clip_id)),
                    ContextAction::Locate => locate_clip.run(clip_id),
                    ContextAction::ToggleStack => set_stack.update(|stack| {
                        for id in &ids {
                            toggle_stack_item(stack, *id);
                        }
                    }),
                    ContextAction::Edit | ContextAction::Rename => {
                        let clip = choice
                            .item
                            .filter(|clip| clip.id == clip_id && ids.len() == 1)
                            .ok_or(t("菜单编辑内容已失效。"))?;
                        let rename = choice.choice == ContextAction::Rename;
                        if !rename && needs_rich_text_editor(&clip) {
                            invoke::<()>(
                                "open_rich_text_editor",
                                &CommandArgs {
                                    request: ClipRequest {
                                        clip_id: clip.id.to_string(),
                                    },
                                },
                            )
                            .await?;
                        } else if !rename && clip.content_kind == ContentKind::Image {
                            set_preview_open.set(Some(clip.id));
                        } else {
                            set_content_editor_id.set(Some(clip.id));
                            set_content_editor_kind.set(clip.content_kind);
                            set_content_editor_title.set(clip.title);
                            set_content_editor_value.set(clip.searchable_text);
                            set_content_editor_rename_only.set(rename);
                            set_content_editor_open.set(true);
                        }
                    }
                    ContextAction::Pin(board) | ContextAction::Unpin(board) => {
                        let command = if matches!(choice.choice, ContextAction::Pin(_)) {
                            "pin_clips"
                        } else {
                            "unpin_clips"
                        };
                        invoke::<()>(
                            command,
                            &CommandArgs {
                                request: PinClipsRequest {
                                    pinboard_id: board.to_string(),
                                    clip_ids: ids.iter().map(ToString::to_string).collect(),
                                },
                            },
                        )
                        .await?;
                        order_revision.update(|revision| *revision = revision.wrapping_add(1));
                        set_notice.set(Some(t("Pinboard 归属已更新，剪贴板历史保持不变。").into()));
                    }
                    ContextAction::Delete => {
                        invoke::<()>(
                            "delete_clips",
                            &CommandArgs {
                                request: ClipsRequest {
                                    clip_ids: ids.iter().map(ToString::to_string).collect(),
                                },
                            },
                        )
                        .await?;
                        set_clips.update(|clips| clips.retain(|clip| !ids.contains(&clip.id)));
                        set_stack.update(|stack| stack.retain(|id| !ids.contains(id)));
                        set_selected_ids.set(HashSet::new());
                        set_selected.set(0);
                        order_revision.update(|revision| *revision = revision.wrapping_add(1));
                        set_notice.set(Some(localized_format!("已删除 {} 项内容；⌘Z 可撤销。", "Deleted {} items. Press ⌘Z to undo.", ids.len())));
                    }
                }
                Ok(())
            }
            .await;
            context_menu_open.set(false);
            set_error.set(outcome.err());
        });
    });
    context_menu_callback.set_value(Some(show_context_menu));

    let save_content = move |_| {
        let clip_id = content_editor_id.get_untracked();
        let kind = content_editor_kind.get_untracked();
        let title = content_editor_title.get_untracked();
        let value = content_editor_value.get_untracked();
        if content_editor_rename_only.get_untracked() {
            let Some(clip_id) = clip_id else {
                return;
            };
            if title.trim().is_empty() {
                set_error.set(Some(t("标题不能为空。").into()));
                return;
            }
            let saved_title = title.trim().to_owned();
            spawn_local(async move {
                match invoke::<()>(
                    "rename_clip",
                    &CommandArgs {
                        request: RenameRequest {
                            clip_id: clip_id.to_string(),
                            title: saved_title.clone(),
                        },
                    },
                )
                .await
                {
                    Ok(()) => {
                        set_clips.update(|items| {
                            if let Some(item) = items.iter_mut().find(|item| item.id == clip_id) {
                                item.title = saved_title;
                            }
                        });
                        set_content_editor_open.set(false);
                        set_content_editor_id.set(None);
                        set_content_editor_rename_only.set(false);
                        set_notice.set(Some(t("标题已更新。").into()));
                        set_error.set(None);
                    }
                    Err(message) => set_error.set(Some(message)),
                }
            });
            return;
        }
        if value.trim().is_empty() {
            set_error.set(Some(t("内容不能为空。").into()));
            return;
        }
        if kind == ContentKind::Color && paste_domain::parse_color_code(&value).is_none() {
            set_error.set(Some(
                t("请输入六位色值：带 #，或至少含一个 A–F 字母。").into(),
            ));
            return;
        }
        let active_board = active_pinboard.get_untracked();
        spawn_local(async move {
            let result = if let Some(clip_id) = clip_id {
                invoke::<ClipItem>(
                    "update_textual_item",
                    &CommandArgs {
                        request: UpdateTextualItemRequest {
                            clip_id: clip_id.to_string(),
                            kind,
                            title,
                            value,
                        },
                    },
                )
                .await
            } else {
                invoke::<ClipItem>(
                    "create_textual_item",
                    &CommandArgs {
                        request: CreateTextualItemRequest { kind, value },
                    },
                )
                .await
            };
            match result {
                Ok(saved) => {
                    let saved_id = saved.id;
                    set_clips.update(|items| {
                        if let Some(index) = items.iter().position(|item| item.id == saved_id) {
                            items[index] = saved.clone();
                            if active_board.is_none() {
                                let saved = items.remove(index);
                                items.insert(0, saved);
                            }
                        } else if active_board.is_none() {
                            items.insert(0, saved);
                        }
                    });
                    set_previews.update(|items| {
                        items.remove(&saved_id);
                    });
                    if let Some(index) = clips
                        .get_untracked()
                        .iter()
                        .position(|item| item.id == saved_id)
                    {
                        set_selected.set(index);
                        set_selection_anchor.set(index);
                        set_selected_ids.set(HashSet::from([saved_id]));
                    }
                    set_content_editor_open.set(false);
                    set_content_editor_id.set(None);
                    set_content_editor_rename_only.set(false);
                    set_notice.set(Some(if clip_id.is_some() {
                        t("内容已更新并重新建立搜索索引。").into()
                    } else {
                        t("新内容已保存到本地历史。").into()
                    }));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let pin_selected = Callback::new(move |pinboard_id: PinboardId| {
        let clip_ids = ordered_selection(
            &clips.get_untracked(),
            &selected_ids.get_untracked(),
            selected.get_untracked(),
        );
        if !clip_ids.is_empty() {
            spawn_local(async move {
                match invoke::<()>(
                    "pin_clips",
                    &CommandArgs {
                        request: PinClipsRequest {
                            pinboard_id: pinboard_id.to_string(),
                            clip_ids: clip_ids
                                .into_iter()
                                .map(|clip_id| clip_id.to_string())
                                .collect(),
                        },
                    },
                )
                .await
                {
                    Ok(()) => {
                        set_pin_menu_open.set(false);
                        set_error.set(None);
                    }
                    Err(message) => set_error.set(Some(message)),
                }
            });
        }
    });

    let unpin_selected = move |_| {
        if search_active.get_untracked() || results_pending.get_untracked() {
            return;
        }
        let pinboard_id = active_pinboard.get_untracked();
        let clip_ids = ordered_selection(
            &clips.get_untracked(),
            &selected_ids.get_untracked(),
            selected.get_untracked(),
        );
        if let Some(pinboard_id) = pinboard_id
            && !clip_ids.is_empty()
        {
            let removed = clip_ids.iter().copied().collect::<HashSet<_>>();
            spawn_local(async move {
                match invoke::<()>(
                    "unpin_clips",
                    &CommandArgs {
                        request: PinClipsRequest {
                            pinboard_id: pinboard_id.to_string(),
                            clip_ids: clip_ids
                                .into_iter()
                                .map(|clip_id| clip_id.to_string())
                                .collect(),
                        },
                    },
                )
                .await
                {
                    Ok(()) => {
                        set_clips.update(|items| {
                            items.retain(|item| !removed.contains(&item.id));
                        });
                        set_selected_ids.set(HashSet::new());
                        set_error.set(None);
                    }
                    Err(message) => set_error.set(Some(message)),
                }
            });
        }
    };

    let rotate_preview_image = Callback::new(move |(clip_id, direction): (ClipId, i8)| {
        if image_edit_loading.get_untracked().contains(&clip_id)
            || ocr_loading.get_untracked().contains(&clip_id)
        {
            return;
        }
        set_image_edit_loading.update(|items| {
            items.insert(clip_id);
        });
        set_error.set(None);
        spawn_local(async move {
            let result = invoke::<ClipItem>(
                "rotate_clip_image",
                &CommandArgs {
                    request: RotateImageRequest {
                        clip_id: clip_id.to_string(),
                        direction,
                    },
                },
            )
            .await;
            match result {
                Ok(updated) => {
                    set_clips.update(|items| {
                        if let Some(item) = items.iter_mut().find(|item| item.id == clip_id) {
                            *item = updated;
                        }
                    });
                    set_previews.update(|items| {
                        items.remove(&clip_id);
                    });
                    set_preview_loading.update(|items| {
                        items.insert(clip_id);
                    });
                    let refreshed = invoke::<PreviewResult>(
                        "get_clip_preview",
                        &CommandArgs {
                            request: PreviewRequest {
                                clip_id: clip_id.to_string(),
                            },
                        },
                    )
                    .await;
                    set_preview_loading.update(|items| {
                        items.remove(&clip_id);
                    });
                    match refreshed {
                        Ok(preview) => {
                            set_previews.update(|items| {
                                items.insert(clip_id, preview);
                            });
                            set_notice.set(Some(t("图片已在本地旋转并保存。").into()));
                            set_error.set(None);
                        }
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_image_edit_loading.update(|items| {
                items.remove(&clip_id);
            });
        });
    });

    let recognize_preview_text = Callback::new(move |clip_id: ClipId| {
        if image_edit_loading.get_untracked().contains(&clip_id)
            || ocr_loading.get_untracked().contains(&clip_id)
        {
            return;
        }
        set_ocr_loading.update(|items| {
            items.insert(clip_id);
        });
        set_error.set(None);
        spawn_local(async move {
            match invoke::<OcrResult>(
                "recognize_clip_text",
                &CommandArgs {
                    request: ClipRequest {
                        clip_id: clip_id.to_string(),
                    },
                },
            )
            .await
            {
                Ok(result) => {
                    set_clips.update(|items| {
                        if let Some(item) = items.iter_mut().find(|item| item.id == clip_id) {
                            *item = result.item;
                        }
                    });
                    set_notice.set(Some(localized_format!("已在本机识别 {} 行、{} 个字符，并加入搜索索引。", "Recognized {} lines and {} characters locally and added them to the search index.",
                        result.line_count, result.character_count
                    )));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_ocr_loading.update(|items| {
                items.remove(&clip_id);
            });
        });
    });

    let open_preview_link = Callback::new(move |clip_id: ClipId| {
        spawn_local(async move {
            match open_link_preview(clip_id).await {
                Ok(()) => {
                    set_notice.set(Some(t("已在 CopyRail 内置浏览器中打开链接。").into()));
                    set_error.set(None);
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    });

    let change_settings = Callback::new(move |opening: bool| {
        if opening {
            set_preview_open.set(None);
        }
        if settings_open.get_untracked() != opening {
            set_settings_open.set(opening);
        }
        if opening {
            refresh_shortcut.run(());
            spawn_local(async move {
                if let Ok(current) =
                    invoke::<PermissionStatus>("get_permission_status", &EmptyArgs {}).await
                {
                    set_permission_status.set(Some(current));
                }
                if let Ok(current) = invoke::<SyncStatus>("get_sync_status", &EmptyArgs {}).await {
                    set_sync_status.set(Some(current));
                }
                if let Ok(conflicts) =
                    invoke::<Vec<SyncConflictView>>("list_sync_conflicts", &EmptyArgs {}).await
                {
                    set_sync_conflicts.set(conflicts);
                }
                if let Ok(conflicts) =
                    invoke::<Vec<SharedConflictView>>("list_shared_conflicts", &EmptyArgs {}).await
                {
                    set_shared_conflicts.set(conflicts);
                }
                if let Ok(current) =
                    invoke::<McpAccessStatus>("get_mcp_access_status", &EmptyArgs {}).await
                {
                    set_mcp_status.set(Some(current));
                }
            });
        }
        set_pin_menu_open.set(false);
        set_pinboard_creator_open.set(false);
        set_pinboard_editor_open.set(false);
        set_filter_menu_open.set(false);
    });

    view! {
        <main
            class="paste-shell"
            class:auxiliary-workspace=auxiliary
            class:native-rail=native_rail
            class:reading-preview=move || preview_visible.get()
            class:expanded-workspace=move || expanded_visible.get()
            class:workspace-ready=move || workspace_ready.get()
            class:dock-short=move || dock_height.get().min(viewport_height.get()) <= 190.0
            style=move || format!("--saved-dock-height:{}px;--background-opacity:{};", dock_height.get(), 1.0 - f64::from(desktop_preferences.get().background_transparency) / 100.0)
            class:compact=move || applied_compact.get()
            class:drag-active=move || native_dragging.get()
            class:native-feedback=move || native_feedback_active.get()
            tabindex="0"
            on:keydown=on_keydown
            on:pointerdown:capture=move |event| {
                let target = event.target().and_then(|target| target.dyn_into::<web_sys::Element>().ok());
                if let Some(target) = target {
                    if target.closest(".pin-menu, .organize-button").ok().flatten().is_none() {
                        set_pin_menu_open.set(false);
                    }
                    if target.closest(".filter-popover, .filter-toggle").ok().flatten().is_none() {
                        set_filter_menu_open.set(false);
                    }
                    if target.closest(".settings-popover, .settings-button, .shortcut-warning, .permission-help-button").ok().flatten().is_none() {
                        set_settings_open.set(false);
                    }
                }
            }
        >
            <div class="dock-surface">
            <div class="drag-region" data-tauri-drag-region></div>
            <header class="toolbar">
                <div class="rail-brand" aria-label="CopyRail">
                    <span aria-hidden="true" inner_html=include_str!("../brand.svg")></span>
                    <strong>"CopyRail"</strong>
                </div>
                <div class="search-wrap">
                    <span class="search-icon" aria-hidden="true">"⌕"</span>
                    <input
                        node_ref=search_input
                        id="history-search"
                        aria-controls="history-results"
                        aria-label=move || t("搜索剪贴板历史")
                        type="search"
                        placeholder=move || t("搜索复制过的内容")
                        prop:value=move || query.get()
                        on:input=move |event| {
                            set_query.set(event_target_value(&event));
                            set_history_offset.set(0);
                            set_selected.set(0);
                            set_selected_ids.set(HashSet::new());
                            set_selection_anchor.set(0);
                        }
                    />
                    <button
                        class="filter-toggle"
                        class:active=move || {
                            active_kind.get().is_some()
                                || active_source.get().is_some()
                                || active_device.get().is_some()
                                || date_days.get().is_some()
                        }
                        type="button"
                        aria-label=move || t("按内容类型筛选")
                        title=move || t("按内容类型筛选")
                        on:click=move |_| {
                            set_filter_menu_open.update(|value| *value = !*value);
                            set_settings_open.set(false);
                            set_pinboard_creator_open.set(false);
                            set_pin_menu_open.set(false);
                        }
                    >
                        {move || {
                            let count = usize::from(active_kind.get().is_some())
                                + usize::from(active_source.get().is_some())
                                + usize::from(active_device.get().is_some())
                                + usize::from(date_days.get().is_some());
                            if count == 0 { t("筛选").into() } else { localized_format!("筛选 {count}", "Filter {count}") }
                        }}
                    </button>
                </div>
                <div class="toolbar-actions">
                    <button
                        class="content-action"
                        type="button"
                        aria-label=move || t("新建文本、链接或颜色")
                        title=move || t("新建文本、链接或颜色")
                        on:click=open_new_item
                    >"＋"</button>
                    {move || {
                        let selection = selected_ids.get();
                        let editable = selection.len() == 1
                            && clips
                                .get()
                                .iter()
                                .any(|clip| {
                                    selection.contains(&clip.id)
                                        && (is_textually_editable(clip.content_kind)
                                            || needs_rich_text_editor(clip)
                                            || clip.content_kind == ContentKind::Image)
                                });
                        editable.then(|| view! {
                            <button
                                class="content-action"
                                type="button"
                                title=move || t("编辑选中内容")
                                on:click=open_content_editor
                            >{move || t("编辑")}</button>
                        })
                    }}
                    {move || (selected_ids.get().len() == 1).then(|| view! {
                        <button
                            class="content-action"
                            type="button"
                            title=move || t("重命名选中项目（⌘R）")
                            on:click=open_rename_editor
                        >{move || t("命名")}</button>
                    })}
                    {move || (selected_ids.get().len() > 1).then(|| view! {
                        <span class="selection-count">{localized_format!("已选 {} 项", "{} selected", selected_ids.get().len())}</span>
                    })}
                    {move || if active_pinboard.get().is_some() && !search_active.get() {
                        view! {
                            <div class="pinboard-item-actions">
                                {move || {
                                    let sortable = query.get().trim().is_empty()
                                        && active_kind.get().is_none()
                                        && active_source.get().is_none()
                                        && active_device.get().is_none()
                                        && date_days.get().is_none()
                                        && selected_ids.get().len() == 1;
                                    sortable.then(|| view! {
                                        <button
                                            class="organize-button"
                                            type="button"
                                            title=move || t("在 Pinboard 中向前移动")
                                            on:click=move |_| move_pinboard_item.run(-1)
                                        >"←"</button>
                                        <button
                                            class="organize-button"
                                            type="button"
                                            title=move || t("在 Pinboard 中向后移动")
                                            on:click=move |_| move_pinboard_item.run(1)
                                        >"→"</button>
                                    })
                                }}
                                <button class="organize-button" type="button" on:click=unpin_selected>
                                    {move || t("移出")}
                                </button>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <button
                                class="organize-button"
                                class:active=move || pin_menu_open.get()
                                type="button"
                                on:click=move |_| {
                                    set_pin_menu_open.update(|value| *value = !*value);
                                    set_settings_open.set(false);
                                    set_pinboard_creator_open.set(false);
                                    set_filter_menu_open.set(false);
                                }
                            >{move || t("归类")}</button>
                        }.into_any()
                    }}
                    <button
                        class="stack-button"
                        class:active=move || !stack.get().is_empty()
                        type="button"
                        title=move || t("清空待粘贴列表（不会删除历史）")
                        on:click=move |_| set_stack.set(Vec::new())
                    >
                        <span>{move || t("顺序粘贴")}</span>
                        <strong>{move || stack.get().len()}</strong>
                    </button>
                    {move || if capture_isolated.get() {
                        view! { <span class="native-test-badge" role="status" title=move || t("合成历史；不访问系统剪贴板或 iCloud")>{move || t("隔离验证")}</span> }.into_any()
                    } else {
                        capture_status_visible.get().then(|| view! {
                            <button class="capture-status" type="button" role="status"
                                class:paused=move || pending_capture_control().is_none() && status.get().is_some_and(|value| value.paused)
                                title=move || t("在设置中管理采集")
                                on:click=move |_| { change_settings.run(true); settings_tab.set("history"); }
                            >{move || match pending_capture_control().as_deref() {
                                Some("pause") => t("正在暂停…"),
                                Some("resume") => t("正在恢复…"),
                                Some(_) => t("正在应用设置…"),
                                None => t("采集已暂停"),
                            }}</button>
                        }).into_any()
                    }}
                    <button
                        class="settings-button"
                        class:active=move || settings_open.get()
                        type="button"
                        aria-label=move || t("采集与隐私设置")
                        title=move || t("采集与隐私设置")
                        on:click=move |_| change_settings.run(!settings_open.get_untracked())
                    >"⚙"</button>

                </div>
                <nav class="pinboards" aria-label="Pinboards">
                    <button
                        class="pinboard"
                        class:active=move || active_pinboard.get().is_none() && !search_active.get()
                        aria-current=move || (active_pinboard.get().is_none() && !search_active.get()).then_some("true")
                        type="button"
                        on:click=move |_| {
                            set_active_pinboard.set(None);
                            set_pinboard_editor_open.set(false);
                            set_history_offset.set(0);
                            set_selected.set(0);
                            set_selected_ids.set(HashSet::new());
                            set_selection_anchor.set(0);
                        }
                    >
                        <span class="pin-dot clipboard"></span>
                        {move || t("剪贴板")}
                    </button>
                    <For
                        each=move || pinboards.get()
                        key=|pinboard| pinboard.id
                        children=move |pinboard| {
                            let color = format!("--pin-color:{}", pinboard.color);
                            let pinboard_id = pinboard.id;
                            view! {
                                <button
                                    class="pinboard"
                                    class:active=move || active_pinboard.get() == Some(pinboard_id) && !search_active.get()
                                    aria-current=move || (active_pinboard.get() == Some(pinboard_id) && !search_active.get()).then_some("true")
                                    class:drop-target=move || drop_target.get().is_some_and(|target| target.pinboard_id == pinboard_id && target.anchor.is_none())
                                    class:drop-before=move || tab_hover.get() == Some((pinboard_id, false))
                                    class:drop-after=move || tab_hover.get() == Some((pinboard_id, true))
                                    data-drop-board=pinboard_id.to_string()
                                    draggable="false"
                                    on:pointerdown=move |event| {
                                        if placement_busy.get_untracked() || event.button() != 0 { return; }
                                        tab_click_suppressed.set(false);
                                        tab_pointer.set(Some((pinboard_id, event.client_x(), event.client_y())));
                                        if let Some(element) = event.current_target().and_then(|target| target.dyn_into::<web_sys::Element>().ok()) {
                                            let _ = element.set_pointer_capture(event.pointer_id());
                                        }
                                    }
                                    on:pointermove=move |event| {
                                        let Some((board, x, y)) = tab_pointer.get_untracked() else { return; };
                                        if tab_drag.get_untracked().is_none() && (event.client_x() - x).abs() < 5 && (event.client_y() - y).abs() < 5 { return; }
                                        tab_drag.set(Some(board));
                                        drag_drop::scroll_at_edge(f64::from(event.client_x()), f64::from(event.client_y()));
                                        tab_hover.set(drag_drop::tab_target(event.client_x(), event.client_y()).filter(|(id, _)| *id != board));
                                    }
                                    on:pointerup=move |event| {
                                        let dragging = tab_drag.get_untracked().is_some();
                                        if dragging {
                                            tab_click_suppressed.set(true);
                                            if let Some(target) = drag_drop::tab_target(event.client_x(), event.client_y()) { drop_pinboard.run(target); }
                                        }
                                        tab_drag.set(None); tab_hover.set(None); tab_pointer.set(None);
                                        if let Some(element) = event.current_target().and_then(|target| target.dyn_into::<web_sys::Element>().ok()) {
                                            let _ = element.release_pointer_capture(event.pointer_id());
                                        }
                                    }
                                    on:pointercancel=move |_| {
                                        tab_drag.set(None); tab_hover.set(None); tab_pointer.set(None);
                                    }
                                    on:lostpointercapture=move |_| {
                                        tab_drag.set(None); tab_hover.set(None); tab_pointer.set(None);
                                    }
                                    type="button"
                                    on:click=move |_| {
                                        if tab_click_suppressed.get_untracked() { tab_click_suppressed.set(false); return; }
                                        set_active_pinboard.set(Some(pinboard_id));
                                        set_pinboard_editor_open.set(false);
                                        set_history_offset.set(0);
                                        set_selected.set(0);
                                        set_selected_ids.set(HashSet::new());
                                        set_selection_anchor.set(0);
                                    }
                                >
                                    <span class="pin-dot" style=color></span>
                                    {pinboard.name}
                                </button>
                            }
                        }
                    />
                    <button
                        class="pinboard add-pinboard"
                        type="button"
                        aria-label=move || t("新建 Pinboard")
                        title=move || t("新建 Pinboard")
                        on:click=move |_| {
                            set_pinboard_creator_open.update(|value| *value = !*value);
                            set_pinboard_editor_open.set(false);
                            set_settings_open.set(false);
                            set_pin_menu_open.set(false);
                            set_filter_menu_open.set(false);
                        }
                    >"＋"</button>
                    {move || search_context.get().scoped_board().map(|pinboard_id| view! {
                        <button
                            class="pinboard manage-pinboard"
                            class:active=move || pinboard_editor_open.get()
                            type="button"
                            aria-label=move || t("编辑当前 Pinboard")
                            title=move || t("重命名、改色或排序")
                            on:click=move |_| {
                                let opening = !pinboard_editor_open.get_untracked();
                                set_pinboard_editor_open.set(opening);
                                if opening
                                    && let Some(pinboard) = pinboards
                                        .get_untracked()
                                        .into_iter()
                                        .find(|item| item.id == pinboard_id)
                                {
                                    set_edit_pinboard_name.set(pinboard.name);
                                    set_edit_pinboard_color.set(pinboard.color);
                                }
                                set_pinboard_creator_open.set(false);
                                set_settings_open.set(false);
                                set_pin_menu_open.set(false);
                                set_filter_menu_open.set(false);
                            }
                        >"•••"</button>
                    })}
                </nav>
            </header>

            {move || filter_menu_open.get().then(|| view! {
                <aside class="filter-popover" aria-label=move || t("组合筛选")>
                    <header>
                        <strong>{move || t("筛选")}</strong>
                        <button type="button" on:click=move |_| {
                            set_active_kind.set(None);
                            set_active_source.set(None);
                            set_active_device.set(None);
                            set_date_days.set(None);
                            set_history_offset.set(0);
                            set_selected.set(0);
                            set_selected_ids.set(HashSet::new());
                            set_selection_anchor.set(0);
                        }>{move || t("清除")}</button>
                    </header>
                    <section>
                        <strong>{move || t("内容")}</strong>
                        <div class="filter-chips">
                            <For
                                each=move || FILTER_KINDS
                                key=|kind| *kind
                                children=move |kind| view! {
                                    <button
                                        class:active=move || active_kind.get() == Some(kind)
                                        type="button"
                                        on:click=move |_| {
                                            set_active_kind.update(|value| {
                                                *value = (*value != Some(kind)).then_some(kind);
                                            });
                                            set_history_offset.set(0);
                                        }
                                    >{move || kind_label(kind)}</button>
                                }
                            />
                        </div>
                    </section>
                    <section>
                        <strong>{move || t("来源应用")}</strong>
                        <div class="filter-chips facets">
                            <For
                                each=move || {
                                    let mut sources = search_facets.get().sources;
                                    sources.truncate(8);
                                    sources
                                }
                                key=|source| source.bundle_identifier.clone()
                                children=move |source| {
                                    let bundle_id = source.bundle_identifier.clone();
                                    let selected_id = bundle_id.clone();
                                    view! {
                                        <button
                                            class:active=move || active_source.get().as_ref() == Some(&selected_id)
                                            type="button"
                                            on:click=move |_| {
                                                let next = bundle_id.clone();
                                                set_active_source.update(|value| {
                                                    *value = (value.as_ref() != Some(&next)).then_some(next);
                                                });
                                                set_history_offset.set(0);
                                            }
                                        >{format!("{} {}", source.display_name, source.item_count)}</button>
                                    }
                                }
                            />
                        </div>
                    </section>
                    <section>
                        <strong>{move || t("设备")}</strong>
                        <div class="filter-chips facets">
                            <For
                                each=move || search_facets.get().devices
                                key=|device| device.id
                                children=move |device| {
                                    let device_id = device.id;
                                    view! {
                                        <button
                                            class:active=move || active_device.get() == Some(device_id)
                                            type="button"
                                            on:click=move |_| {
                                                set_active_device.update(|value| {
                                                    *value = (*value != Some(device_id)).then_some(device_id);
                                                });
                                                set_history_offset.set(0);
                                            }
                                        >{format!("{} {}", device.display_name, device.item_count)}</button>
                                    }
                                }
                            />
                        </div>
                    </section>
                    <section>
                        <strong>{move || t("时间")}</strong>
                        <div class="filter-chips">
                            {[(1_i64, "最近 24 小时"), (7, "最近 7 天"), (30, "最近 30 天")].into_iter().map(|(days, label)| view! {
                                <button
                                    class:active=move || date_days.get() == Some(days)
                                    type="button"
                                    on:click=move |_| {
                                        set_date_days.update(|value| {
                                            *value = (*value != Some(days)).then_some(days);
                                        });
                                        set_history_offset.set(0);
                                    }
                                >{move || t(label)}</button>
                            }).collect_view()}
                        </div>
                    </section>
                </aside>
            })}

            {move || pinboard_creator_open.get().then(|| view! {
                <aside class="pinboard-creator" aria-label=move || t("新建 Pinboard")>
                    <label class="pinboard-name-field"><span>{move || t("名称")}</span><input type="text" maxlength="80" placeholder=move || t("分类名称")
                        prop:value=move || new_pinboard_name.get()
                        on:input=move |event| set_new_pinboard_name.set(event_target_value(&event)) /></label>
                    <label class="pinboard-color-field"><span>{move || t("颜色")}</span><input type="color" aria-label=move || t("Pinboard 颜色")
                        prop:value=move || new_pinboard_color.get()
                        on:input=move |event| set_new_pinboard_color.set(event_target_value(&event)) /></label>
                    <button type="button" on:click=create_pinboard>{move || t("创建")}</button>
                </aside>
            })}

            {move || pinboard_editor_open.get().then(|| view! {
                <aside class="pinboard-editor" aria-label=move || t("编辑 Pinboard")>
                    <header>
                        <strong>{move || t("编辑分类")}</strong>
                        <button type="button" on:click=move |_| set_pinboard_editor_open.set(false)>
                            "×"
                        </button>
                    </header>
                    <div class="pinboard-editor-fields">
                        <label class="pinboard-name-field"><span>{move || t("名称")}</span><input type="text" maxlength="80" placeholder=move || t("分类名称")
                            prop:value=move || edit_pinboard_name.get()
                            on:input=move |event| set_edit_pinboard_name.set(event_target_value(&event)) /></label>
                        <label class="pinboard-color-field"><span>{move || t("颜色")}</span><input type="color" aria-label=move || t("Pinboard 颜色")
                            prop:value=move || edit_pinboard_color.get()
                            on:input=move |event| set_edit_pinboard_color.set(event_target_value(&event)) /></label>
                    </div>
                    <div class="pinboard-editor-actions">
                        <div>
                            <button type="button" title=move || t("向前移动") on:click=move |_| move_pinboard.run(-1)>
                                "←"
                            </button>
                            <button type="button" title=move || t("向后移动") on:click=move |_| move_pinboard.run(1)>
                                "→"
                            </button>
                        </div>
                        <div>
                            <button class="delete-pinboard" type="button" on:click=delete_pinboard>
                                {move || t("删除")}
                            </button>
                            <button class="save-pinboard" type="button" on:click=save_pinboard>
                                {move || t("保存")}
                            </button>
                        </div>
                    </div>
                </aside>
            })}

            {move || pin_menu_open.get().then(|| view! {
                <aside class="pin-menu" aria-label=move || t("固定到 Pinboard")>
                    <strong>{move || t("固定到")}</strong>
                    {move || if pinboards.get().is_empty() {
                        view! { <span>{move || t("先新建一个 Pinboard")}</span> }.into_any()
                    } else {
                        view! {
                            <div>
                                <For
                                    each=move || pinboards.get()
                                    key=|pinboard| pinboard.id
                                    children=move |pinboard| {
                                        let id = pinboard.id;
                                        let color = format!("--pin-color:{}", pinboard.color);
                                        view! {
                                            <button type="button" on:click=move |_| pin_selected.run(id)>
                                                <span class="pin-dot" style=color></span>
                                                {pinboard.name}
                                            </button>
                                        }
                                    }
                                />
                            </div>
                        }.into_any()
                    }}
                </aside>
            })}

            <dialog node_ref=content_editor_dialog class="content-editor-backdrop"
                aria-labelledby="content-editor-heading" aria-describedby="content-editor-description"
                on:cancel=move |event: web_sys::Event| {
                    event.prevent_default();
                    set_content_editor_open.set(false);
                    set_content_editor_id.set(None);
                    set_content_editor_rename_only.set(false);
                }
            >
                {move || content_editor_open.get().then(|| view! {
                    <aside class="content-editor">
                        <header>
                            <div>
                                <strong id="content-editor-heading">{move || if content_editor_id.get().is_some() {
                                    if content_editor_rename_only.get() {
                                        t("重命名")
                                    } else {
                                        t("编辑内容")
                                    }
                                } else {
                                    t("新建内容")
                                }}</strong>
                                <span id="content-editor-description">{move || if content_editor_rename_only.get() {
                                    t("只修改显示标题，不改动原始剪贴板内容。")
                                } else {
                                    t("内容仅保存在本机，保存后会立即更新搜索索引。")
                                }}</span>
                            </div>
                            <button type="button" aria-label=move || t("关闭编辑器") on:click=move |_| {
                                set_content_editor_open.set(false);
                                set_content_editor_id.set(None);
                                set_content_editor_rename_only.set(false);
                            }>"×"</button>
                        </header>
                        {move || (!content_editor_rename_only.get()).then(|| view! {
                            <div class="content-kind-picker" aria-label=move || t("内容类型")>
                                {[(ContentKind::Text, "文本"), (ContentKind::Link, "链接"), (ContentKind::Color, "颜色")]
                                    .into_iter()
                                    .map(|(kind, label)| view! {
                                        <button
                                            class:active=move || content_editor_kind.get() == kind
                                            type="button"
                                            on:click=move |_| {
                                                set_content_editor_kind.set(kind);
                                                if kind == ContentKind::Color
                                                    && paste_domain::parse_color_code(&content_editor_value.get_untracked()).is_none()
                                                {
                                                    set_content_editor_value.set("#ff9500".into());
                                                }
                                            }
                                        >{move || t(label)}</button>
                                    })
                                    .collect_view()}
                            </div>
                        })}
                        {move || content_editor_id.get().is_some().then(|| view! {
                            <label class="content-title-field">
                                <span>{move || t("标题")}</span>
                                <input
                                    type="text"
                                    autofocus=move || content_editor_rename_only.get()
                                    maxlength="80"
                                    placeholder=move || t("留空则使用内容首行")
                                    prop:value=move || content_editor_title.get()
                                    on:input=move |event| {
                                        set_content_editor_title.set(event_target_value(&event));
                                    }
                                />
                            </label>
                        })}
                        {move || (!content_editor_rename_only.get()).then(|| {
                            if content_editor_kind.get() == ContentKind::Color {
                                view! {
                                <div class="color-edit-row">
                                    <input
                                        type="color"
                                        aria-label=move || t("选择颜色")
                                        prop:value=move || {
                                            let value = content_editor_value.get();
                                            crate::card_visual::color_swatch(ContentKind::Color, &value)
                                                .map_or_else(|| "#ff9500".into(), |(hex, _)| hex)
                                        }
                                        on:input=move |event| {
                                            set_content_editor_value.set(event_target_value(&event));
                                        }
                                    />
                                    <input
                                        type="text"
                                        autofocus=true
                                        aria-label=move || t("十六进制颜色值")
                                        maxlength="7"
                                        placeholder="#ff9500"
                                        prop:value=move || content_editor_value.get()
                                        on:input=move |event| {
                                            set_content_editor_value.set(event_target_value(&event));
                                        }
                                    />
                                </div>
                                }.into_any()
                            } else {
                                view! {
                                <textarea
                                    autofocus=true
                                    aria-label=move || t("内容")
                                    maxlength="4194304"
                                    spellcheck=move || (content_editor_kind.get() == ContentKind::Text).to_string()
                                    placeholder=move || if content_editor_kind.get() == ContentKind::Link {
                                        "https://example.com"
                                    } else {
                                        t("输入要保存的文本…")
                                    }
                                    prop:value=move || content_editor_value.get()
                                    on:input=move |event| {
                                        set_content_editor_value.set(event_target_value(&event));
                                    }
                                ></textarea>
                                }.into_any()
                            }
                        })}
                        {move || error.get().map(|message| view! {
                            <p class="content-editor-error" role="alert">{move || t(&message).to_owned()}</p>
                        })}
                        <footer>
                            <span>{move || if content_editor_rename_only.get() {
                                t("标题可用于搜索；原始格式和内容保持不变。")
                            } else if content_editor_id.get().is_some() {
                                t("保存会保留 Pinboard 归属，并把编辑后的项目移到历史最前。")
                            } else {
                                t("新项目的标题会自动取内容首行。")
                            }}</span>
                            <div>
                                <button type="button" on:click=move |_| {
                                set_content_editor_open.set(false);
                                set_content_editor_id.set(None);
                                set_content_editor_rename_only.set(false);
                            }>{move || t("取消")}</button>
                            <button class="save-content" type="button" on:click=save_content>
                                {move || if content_editor_rename_only.get() { t("重命名") } else { t("保存") }}
                            </button>
                            </div>
                        </footer>
                    </aside>
                })}
            </dialog>

            <p class="sr-only" id="history-keyboard-help">
                {move || localized_format!("当前载入 {} 项。左右箭头选择，Shift 扩展选择，Home 和 End 到已载入内容首尾。Space 预览，F2 进入卡片操作；Tab 切换按钮，Esc 或 F2 返回卡片。", "{} items loaded. Use Left and Right to select, Shift to extend selection, Home and End for first and last loaded items, Space to preview, F2 for card actions, Tab between buttons, and Esc or F2 to return to the card.", clips.with(Vec::len))}
            </p>
            <section class="timeline" id="history-results" node_ref=results_view tabindex="0"
                role=move || if clips.with(Vec::is_empty) { "region" } else { "grid" }
                aria-multiselectable=move || (!clips.with(Vec::is_empty)).then_some("true")
                aria-activedescendant=move || {
                    if !results_have_focus.get() || results_pending.get() { return None; }
                    clips.with(|items| items.get(selected.get()).map(|item| format!("clip-card-{}", item.id)))
                }
                aria-describedby="history-keyboard-help"
                aria-keyshortcuts="F2 Home End"
                title=move || t("左右箭头选择 · Space 预览 · F2 卡片操作")
                on:focus=move |_| results_have_focus.set(true)
                on:blur=move |_| results_have_focus.set(false)
                data-drop-pinboard=move || (!search_active.get() && !results_pending.get()).then(|| active_pinboard.get().map(|id| id.to_string())).flatten()
                on:pointerdown:capture=move |event| trace_gesture.run(gesture_trace::pointer(GesturePhase::RootPointerDown, None, &event))
                on:pointermove:capture=move |event| { if event.buttons() != 0 || card_press.get_untracked().is_some() { trace_gesture.run(gesture_trace::pointer(GesturePhase::RootPointerMove, None, &event)); } }
                on:pointerup:capture=move |event| trace_gesture.run(gesture_trace::pointer(GesturePhase::RootPointerUp, None, &event))
                on:mousedown:capture=move |event| trace_gesture.run(gesture_trace::mouse(GesturePhase::RootMouseDown, &event))
                on:mousemove:capture=move |event| { if event.buttons() != 0 { trace_gesture.run(gesture_trace::mouse(GesturePhase::RootMouseMove, &event)); } }
                on:mouseup:capture=move |event| trace_gesture.run(gesture_trace::mouse(GesturePhase::RootMouseUp, &event))
                aria-label=move || if search_active.get() { t("搜索结果") } else { t("剪贴板时间线") }
                aria-busy=move || (placement_busy.get() || results_pending.get()).to_string()
                class:drop-append=move || drop_target.get().is_some_and(|target| Some(target.pinboard_id) == active_pinboard.get() && target.anchor.is_none())
            >
                <Show when=move || !clips.with(Vec::is_empty) fallback=move || view! {
                        <div class="empty-state">
                            <div class="empty-copy" role="status">
                            <div class="empty-mark" aria-hidden="true" inner_html=include_str!("../brand.svg")></div>
                            <div class="empty-message">
                                <strong>{move || if results_pending.get() { t("正在加载内容…") } else if error.get().is_some() { t("暂时无法加载内容") } else if search_active.get() { t("没有找到匹配内容") } else if active_pinboard.get().is_some() { t("给这个分类放入第一条内容") } else { t("留住每一次有用的复制") }}</strong>
                                <span>{move || if results_pending.get() { t("结果更新后即可选择，不会操作上一次查询的内容。") } else if error.get().is_some() { t("请查看错误提示，稍后将自动重试。") } else if search_active.get() { t("已搜索全部历史与 Pinboard；试试其他关键词或清除筛选。") } else if active_pinboard.get().is_some() { t("从历史或其他 Pinboard 拖入便签，也可以新建内容。") } else if status.get().is_some_and(|state| state.isolated) { t("当前仅使用合成数据，不监听系统剪贴板或连接 iCloud。") } else if status.get().is_some_and(|state| state.paused) { t("剪贴板采集已暂停，恢复后才会收集新内容。") } else { t("新复制的内容将保存在此处，机密与瞬态内容默认跳过。") }}</span>
                            </div>
                            </div>
                            <Show when=move || !results_pending.get() && error.get().is_none()>
                                {move || if search_active.get() {
                                    view! { <button class="empty-action content-action" type="button" on:click=move |_| {
                                        set_query.set(String::new());
                                        set_active_kind.set(None); set_active_source.set(None);
                                        set_active_device.set(None); set_date_days.set(None);
                                        set_history_offset.set(0); set_selected.set(0);
                                        set_selected_ids.set(HashSet::new()); set_selection_anchor.set(0);
                                        if let Some(input) = search_input.get() { let _ = input.focus(); }
                                    }>{move || t("清除搜索与筛选")}</button> }.into_any()
                                } else {
                                    view! { <button class="empty-action content-action" type="button" on:click=open_new_item>{move || t("＋ 新建内容")}</button> }.into_any()
                                }}
                            </Show>
                        </div>
                    }>
                        <div class="card-track" role="row">
                            <For
                                each=move || clips.get().into_iter().enumerate()
                                key=|(index, clip)| (*index, clip.id)
                                children=move |(index, clip)| {
                                    let clip_id = clip.id;
                                    let live_clip = Memo::new(move |_| clips.with(|items| items.iter().find(|item| item.id == clip_id).cloned()).unwrap_or_else(|| clip.clone()));
                                    view! {
                                        <ClipCard
                                            source=Signal::derive(move || live_clip.with(|item| item.source.clone()))
                                            source_icon=Signal::derive(move || live_clip.with(|item| source_icons.with(|cache| cache.peek(&item.source.bundle_identifier).cloned())))
                                            age=Signal::derive(move || clips.with(|_| relative_time(live_clip.with(|item| item.last_copied_at))))
                                            stacked=Signal::derive(move || stack.get().contains(&clip_id))
                                            on_activate=activate_clip
                                            clip=Signal::derive(move || live_clip.get())
                                            selected=Signal::derive(move || selected_ids.get().contains(&clip_id))
                                            keyboard_active=Signal::derive(move || selected.get() == index)
                                            number=index + 1
                                            on_drag=drag_selection
                                            on_context_menu=show_context_menu
                                            card_press=card_press
                                            trace_gesture=trace_gesture
                                            drop_position=Signal::derive(move || drop_target.get().filter(|target| target.anchor == Some(clip_id)).map(|target| target.after))
                                            on_locate=locate_clip
                                            locatable=Signal::derive(move || {
                                                !query.get().trim().is_empty()
                                                    || active_pinboard.get().is_some()
                                                    || active_kind.get().is_some()
                                                    || active_source.get().is_some()
                                                    || active_device.get().is_some()
                                                    || date_days.get().is_some()
                                                    || history_offset.get() > 0
                                            })
                                            on_select=Callback::new(move |(meta, shift)| {
                                                if preview_open.get_untracked().is_some() { set_preview_open.set(Some(clip_id)); }
                                                focus_results.run(());
                                                set_selected.set(index);
                                                if shift {
                                                    set_selected_ids.set(selection_range(
                                                        &clips.get_untracked(),
                                                        selection_anchor.get_untracked(),
                                                        index,
                                                    ));
                                                } else if meta {
                                                    set_selection_anchor.set(index);
                                                    set_selected_ids.update(|ids| {
                                                        if ids.contains(&clip_id) && ids.len() > 1 {
                                                            ids.remove(&clip_id);
                                                        } else {
                                                            ids.insert(clip_id);
                                                        }
                                                    });
                                                } else {
                                                    set_selection_anchor.set(index);
                                                    set_selected_ids.set(HashSet::from([clip_id]));
                                                }
                                            })
                                            on_toggle_stack=toggle_stack
                                            preview_asset=Signal::derive(move || {
                                                previews
                                                    .get()
                                                    .get(&clip_id)
                                                    .filter(|preview| preview.media_type.starts_with("image/"))
                                                    .cloned()
                                            })
                                        />
                                    }
                                }
                            />
                        </div>
                </Show>
            </section>

            {move || error.get().map(|message| {
                let permission_required = message.contains("自动粘贴需要") && message.contains("辅助功能");
                view! {
                    <div class="error-banner" role="status">
                        <span>{move || t(&message).to_owned()}</span>
                        {permission_required.then(|| view! {
                            <button class="permission-help-button" type="button" on:click=move |_| {
                                change_settings.run(true);
                                settings_tab.set("general");
                                settings_focus_target.set(Some("permission"));
                            }>{move || t("设置自动粘贴")}</button>
                        })}
                    </div>
                }
            })}
            {move || (error.get().is_none()).then(|| notice.get()).flatten().map(|message| view! {
                <div class="notice-banner" role="status">{move || t(&message).to_owned()}</div>
            })}
            {move || status.get().and_then(|value| value.last_error).map(|message| view! {
                <div class="error-banner capture-error" role="status">{move || t(&message).to_owned()}</div>
            })}
            </div>
            {move || (settings_open.get() && !native_rail).then(|| view! {
                <aside class="settings-popover" role="dialog" aria-label=move || t("CopyRail 设置")>
                    <header><strong>{move || t("设置")}</strong><button type="button" aria-label=move || t("关闭设置") on:click=move |_| set_settings_open.set(false)>"×"</button></header>
                    <div class="settings-layout">
                    <nav class="settings-nav" aria-label=move || t("设置分类")>
                        <button type="button" class:active=move || settings_tab.get() == "general" aria-current=move || (settings_tab.get() == "general").then_some("page") on:click=move |_| settings_tab.set("general")>{move || t("通用")}</button>
                        <button type="button" class:active=move || settings_tab.get() == "shortcuts" aria-current=move || (settings_tab.get() == "shortcuts").then_some("page") on:click=move |_| { settings_tab.set("shortcuts"); settings_focus_target.set(Some("shortcuts")); }>{move || t("快捷键")}</button>
                        <button type="button" class:active=move || settings_tab.get() == "history" aria-current=move || (settings_tab.get() == "history").then_some("page") on:click=move |_| settings_tab.set("history")>{move || t("历史与隐私")}</button>
                        <button type="button" class:active=move || settings_tab.get() == "backup" aria-current=move || (settings_tab.get() == "backup").then_some("page") on:click=move |_| settings_tab.set("backup")>{move || t("备份")}</button>
                        <button type="button" class:active=move || settings_tab.get() == "advanced" aria-current=move || (settings_tab.get() == "advanced").then_some("page") on:click=move |_| settings_tab.set("advanced")>{move || t("高级")}</button>
                    </nav><div class="settings-content">
                    <div class="settings-page" data-settings-page="general" hidden=move || settings_tab.get() != "general"><h2>{move || t("通用")}</h2><p class="settings-description">{move || t("启动、显示与粘贴")}</p>
                    <label class="language-setting">
                        <span><strong>{move || t("语言")}</strong><small>{move || if language_saving.get() { t("正在保存语言…") } else { t("立即保存，无需重启。") }}</small></span>
                        <select class="language-select" aria-label=move || t("界面语言")
                            prop:value=move || { language_saving.get(); desktop_preferences.get().language.code() }
                            disabled=move || language_saving.get() || settings_saving.get() on:change=change_language>
                            <option value="system">{move || t("跟随系统")}</option>
                            <option value="zh-CN">"简体中文"</option>
                            <option value="en">"English"</option>
                        </select>
                    </label>
                    <Show when=move || language_error.get()><p class="language-error" role="status">{move || t("无法保存语言，请重试。")}</p></Show>
                    <label class="appearance-setting">
                        <span><strong>{move || t("背景透明度")}</strong><small>{move || if appearance_saving.get() { t("正在保存外观…") } else { t("调整后自动保存，应用于所有窗口。") }}</small></span>
                        <div class="appearance-slider">
                            <div><span>{move || t("不透明")}</span><output>{move || format!("{}%", desktop_preferences.get().background_transparency)}</output><span>{move || t("通透")}</span></div>
                            <input type="range" min="0" max="100" step="1" aria-label=move || t("背景透明度")
                                prop:value=move || desktop_preferences.get().background_transparency
                                disabled=move || settings_saving.get() on:input=change_transparency />
                        </div>
                    </label>
                    {move || appearance_error.get().then(|| view! { <p class="appearance-error" role="alert">{move || t("外观保存失败，请重试。")}</p> })}
                    <label class="toggle-setting">
                        <span>{move || t("登录时自动启动")}</span>
                        <input
                            type="checkbox"
                            prop:checked=move || desktop_preferences.get().launch_at_login
                            on:change=move |event| set_desktop_preferences.update(|value| {
                                value.launch_at_login = event_target_checked(&event);
                            })
                        />
                    </label>
                    <label class="toggle-setting">
                        <span>{move || t("紧凑卡片布局")}</span>
                        <input
                            type="checkbox"
                            prop:checked=move || desktop_preferences.get().compact_mode
                            on:change=move |event| set_desktop_preferences.update(|value| {
                                value.compact_mode = event_target_checked(&event);
                            })
                        />
                    </label>
                    <section class="queue-help" aria-label=move || t("顺序粘贴说明")>
                        <strong>{move || t("顺序粘贴")}</strong>
                        <p>{move || t("点卡片上的「＋」按顺序加入待粘贴列表。列表有内容时，回车优先粘贴第一条；再次唤起后可继续下一条。")}</p>
                        <p>{move || t("主界面的数字表示剩余条数。点击「顺序粘贴」清空列表，不会删除历史；仅复制或粘贴请求失败时不会移出该条。")}</p>
                    </section>
                    <section class="permission-settings" aria-label=move || t("直接粘贴权限") tabindex="-1" node_ref=permission_section>
                        <div class="permission-summary">
                            <div>
                                <strong>{move || t("直接粘贴")}</strong>
                                <span>{move || t("复制无需授权；向目标应用发送 ⌘V 需要 macOS 辅助功能权限。")}</span>
                            </div>
                            <span
                                class="permission-state"
                                class:granted=move || permission_status
                                    .get()
                                    .is_some_and(|current| current.accessibility_trusted)
                            >
                                {move || match permission_status.get() {
                                    Some(current) if current.accessibility_trusted => t("已授权"),
                                    Some(_) => t("待授权"),
                                    None => t("检测中"),
                                }}
                            </span>
                        </div>
                        {move || permission_status.get().and_then(|current| current.app_path).map(|path| view! {
                            <p class="permission-app-path">{move || t("当前运行的应用：")}<code>{path}</code></p>
                        })}
                        <details class="permission-troubleshooting"><summary>{move || t("授权故障排查")}</summary><p class="permission-help">{move || t("系统开关已开启却仍显示待授权？内测更新后，旧授权可能仍绑定旧签名。先退出 CopyRail，在辅助功能列表选中旧 CopyRail，点“−”移除，再点“＋”添加上方路径的应用。重新打开并点“重新检测”。仅搬到 Applications 不会更新旧授权；授权后请回到目标输入框重新唤起，不会补发上次粘贴。")}</p></details>
                        <div class="permission-actions">
                            {move || (!permission_status
                                .get()
                                .is_some_and(|current| current.accessibility_trusted))
                                .then(|| view! {
                                    <button type="button" on:click=request_accessibility>
                                        {move || t("授权直接粘贴")}
                                    </button>
                                })}
                            <button type="button" on:click=refresh_accessibility>{move || t("重新检测")}</button>
                        </div>
                    </section>
                    </div>
                    <div class="settings-page" data-settings-page="shortcuts" hidden=move || settings_tab.get() != "shortcuts"><h2>{move || t("快捷键")}</h2><p class="settings-description">{move || t("快速打开与键盘操作")}</p>
                    <section class="permission-settings shortcut-settings" aria-label=move || t("全局快捷键") tabindex="-1" node_ref=shortcut_section>
                        <div class="permission-summary">
                            <div>
                                <strong>{move || t("全局快捷键 ⇧⌘V")}</strong>
                                <span>{move || match shortcut_status.get() {
                                    Some(current) if current.isolated => t("隔离验证不会注册系统快捷键。"),
                                    Some(current) if current.registered => t("快捷键已注册；也可通过菜单栏「显示 CopyRail」打开。"),
                                    Some(_) => t("快捷键暂不可用，可能已被其他软件占用。可从菜单栏「显示 CopyRail」打开；释放此组合键后点击「重新启用」。"),
                                    None => t("正在读取快捷键状态；菜单栏入口仍可使用。"),
                                }}</span>
                            </div>
                            <span class="permission-state shortcut-state" class:granted=move || shortcut_status.get().is_some_and(|value| value.registered) role="status">
                                {move || if shortcut_retrying.get() { t("处理中") } else { match shortcut_status.get() {
                                    Some(current) if current.isolated => t("隔离未注册"),
                                    Some(current) if current.registered => t("已注册"),
                                    Some(_) => t("未启用"),
                                    None => t("未知"),
                                }}}
                            </span>
                        </div>
                        {move || shortcut_request_error.get().map(|message| view! { <p class="shortcut-request-error" role="status">{move || t(&message).to_owned()}</p> })}
                        {move || shortcut_status.get().and_then(|value| value.error).map(|message| view! {
                            <details class="shortcut-details"><summary>{move || t("技术详情")}</summary><code>{move || t(&message).to_owned()}</code></details>
                        })}
                        <div class="permission-actions">
                            <button class="shortcut-refresh" type="button" disabled=move || shortcut_retrying.get() on:click=move |_| refresh_shortcut.run(())>{move || t("重新检测")}</button>
                            <button class="shortcut-retry" type="button" disabled=move || shortcut_retrying.get() || shortcut_status.get().is_none_or(|value| value.isolated || value.registered) on:click=retry_shortcut>{move || if shortcut_retrying.get() { t("正在启用…") } else { t("重新启用") }}</button>
                        </div>
                    </section>
                    <dl class="keyboard-reference" aria-label=move || t("界面快捷键")>
                        <div><dt>{move || t("选择内容")}</dt><dd><kbd>"← / →"</kbd></dd></div>
                        <div><dt>{move || t("预览 / 关闭预览")}</dt><dd><kbd>"Space"</kbd></dd></div>
                        <div><dt>{move || t("粘贴")}</dt><dd><kbd>"Return"</kbd></dd></div>
                        <div><dt>{move || t("以纯文本粘贴")}</dt><dd><kbd>"⇧ Return"</kbd></dd></div>
                        <div><dt>{move || t("复制")}</dt><dd><kbd>"⌘ C"</kbd></dd></div>
                        <div><dt>{move || t("加入 / 移出顺序粘贴")}</dt><dd><kbd>"⌘ Return"</kbd></dd></div>
                        <div><dt>{move || t("关闭预览、设置或主界面")}</dt><dd><kbd>"Esc"</kbd></dd></div>
                    </dl>
                    </div>
                    <div class="settings-page" data-settings-page="history" hidden=move || settings_tab.get() != "history"><h2>{move || t("历史与隐私")}</h2><p class="settings-description">{move || t("保留范围与忽略规则")}</p>
                    <section class="capture-settings" aria-label=move || t("剪贴板采集")>
                        <div><strong>{move || t("剪贴板采集")}</strong><p>{move || t("暂停期间不保存新复制的内容，15 分钟后自动恢复。")}</p></div>
                        {move || if capture_isolated.get() {
                            view! { <span>{move || t("隔离验证不采集系统剪贴板。")}</span> }.into_any()
                        } else { view! {
                            <button class="status-button" type="button"
                                class:paused=move || pending_capture_control().is_none() && status.get().is_some_and(|value| value.paused)
                                class:pending=move || pending_capture_control().is_some()
                                aria-disabled=move || if pending_capture_control().is_some() { "true" } else { "false" }
                                aria-busy=move || if pending_capture_control().is_some() { "true" } else { "false" }
                                title=move || if pending_capture_control().is_some() { t("尚未确认生效；等待当前读取或写入结束，请暂勿复制敏感内容。") } else { t("控制剪贴板采集") }
                                on:click=move |event| if status.get_untracked().is_some_and(|value| value.paused) { resume(event) } else { pause(event) }
                            >
                                {move || match pending_capture_control().as_deref() {
                                    Some("pause") => t("正在暂停…").into(),
                                    Some("resume") => t("正在恢复…").into(),
                                    Some(_) => t("正在应用设置…").into(),
                                    None => status.get().filter(|value| value.paused).as_ref().map_or_else(|| t("暂停 15 分钟").into(), pause_label),
                                }}
                            </button>
                        }.into_any() }}
                    </section>
                    <label>
                        <span>{move || t("最多保留天数")}</span>
                        <input
                            type="number"
                            min="1"
                            placeholder=move || t("永久")
                            prop:value=move || retention_days.get()
                            on:input=move |event| set_retention_days.set(event_target_value(&event))
                        />
                    </label>
                    <label>
                        <span>{move || t("最多保留未固定项目")}</span>
                        <input
                            type="number"
                            min="1"
                            placeholder=move || t("不限")
                            prop:value=move || retention_items.get()
                            on:input=move |event| set_retention_items.set(event_target_value(&event))
                        />
                    </label>
                    <label class="excluded-apps">
                        <span>{move || t("忽略这些应用（每行一个 Bundle ID）")}</span>
                        <textarea
                            placeholder="com.example.password-manager"
                            prop:value=move || excluded_apps.get()
                            on:input=move |event| set_excluded_apps.set(event_target_value(&event))
                        ></textarea>
                    </label>
                    <label class="toggle-setting">
                        <span>{move || t("屏幕共享时隐藏内容")}</span>
                        <input
                            type="checkbox"
                            prop:checked=move || desktop_preferences.get().screen_share_protection
                            on:change=move |event| set_desktop_preferences.update(|value| {
                                value.screen_share_protection = event_target_checked(&event);
                            })
                        />
                    </label>
                    <p>{move || t("固定到 Pinboard 的内容不会被保留策略清理。机密和瞬态剪贴板类型始终默认跳过。")}</p>
                    </div>
                    <div class="settings-page" data-settings-page="backup" hidden=move || settings_tab.get() != "backup"><h2>{move || t("备份")}</h2><p class="settings-description">{move || t("导出与恢复本地数据")}</p>
                    <section class="backup-settings" aria-label=move || t("本地备份")>
                        <div>
                            <strong>{move || t("本地备份")}</strong>
                            <span>{move || t("包含历史、Pinboards 与本地设置；不上传到云端。")}</span>
                        </div>
                        <div class="backup-actions">
                            <button type="button" on:click=export_backup>{move || t("导出备份")}</button>
                            <button class="restore-backup" type="button" on:click=restore_backup>
                                {move || t("恢复备份")}
                            </button>
                        </div>
                    </section>
                    </div>
                    <div class="settings-page" data-settings-page="advanced" hidden=move || settings_tab.get() != "advanced"><h2>{move || t("高级")}</h2><p class="settings-description">{move || t("本机集成与实验功能")}</p>
                    <section class="sync-settings" aria-label=move || t("iCloud 同步状态")>
                        <div class="sync-heading">
                            <div>
                                <strong>{move || t("iCloud 同步")}</strong>
                                <span>{move || match sync_status.get() {
                                    Some(current) if current.enabled && current.blocked_reason.is_some() => t(&current.blocked_reason.unwrap_or_default()).to_owned(),
                                    Some(current) if current.pending_shared_downloads > 0 => {
                                        localized_format!("{} 项共享变更等待本地整合 · 完整共享同步仍在建设中", "{} shared changes awaiting local integration · Full sharing is still in development", current.pending_shared_downloads)
                                    }
                                    Some(current) if current.pending_conflicts > 0 => {
                                        localized_format!("{} 个并发编辑待处理 · {} 项待发送", "{} conflicts to resolve · {} changes pending", current.pending_conflicts, current.pending_changes)
                                    }
                                    Some(current) if current.pending_dependencies > 0 => {
                                        localized_format!("{} 项固定关系等待内容到齐 · {} 项待发送", "{} pin assignments awaiting content · {} changes pending", current.pending_dependencies, current.pending_changes)
                                    }
                                    Some(current) if current.syncing => {
                                        localized_format!("正在安全同步 · {} 项待发送", "Syncing securely · {} changes pending", current.pending_changes)
                                    }
                                    Some(current) if current.cloud_transport_configured && current.enabled => {
                                        current.last_success_at_ms.map_or_else(
                                            || localized_format!("CloudKit 已配置 · 待发送 {} 项", "CloudKit configured · {} changes pending", current.pending_changes),
                                            |last_success| localized_format!("上次同步 {} · 待发送 {} 项", "Last synced {} · {} changes pending", relative_timestamp_ms(last_success), current.pending_changes),
                                        )
                                    }
                                    Some(current) if current.enabled => current.blocked_reason.map(|reason| t(&reason).to_owned()).unwrap_or_else(|| {
                                        localized_format!("同步已选择启用 · {} 项待发送", "Sync enabled · {} changes pending", current.pending_changes)
                                    }),
                                    Some(current) if current.local_outbox_ready => {
                                        localized_format!("已关闭 · 本地队列保留 {} 项，不连接 CloudKit", "Disabled · {} changes kept locally, no CloudKit connection", current.pending_changes)
                                    }
                                    Some(_) => t("本地同步队列不可用").into(),
                                    None => t("正在检查本地同步状态…").into(),
                                }}</span>
                            </div>
                            <input
                                aria-label=move || t("启用 iCloud 同步")
                                type="checkbox"
                                prop:checked=move || sync_status.get().is_some_and(|value| value.enabled)
                                on:change=toggle_cloud_sync
                            />
                        </div>
                        {move || sync_status.get().filter(|s| s.pending_shared_conflicts > 0).map(|s| view! {
                            <p role="status">{localized_format!("{} 个共享并发编辑待选择，双方内容均已保留。", "{} shared conflicts need a choice. Both versions have been preserved.", s.pending_shared_conflicts)}</p>
                        })}
                    </section>
                    <SyncConflictList conflicts=sync_conflicts resolving=resolving_conflicts on_resolve=resolve_sync_conflict />
                    <SharedConflictList conflicts=shared_conflicts resolving=resolving_shared_conflicts on_resolve=resolve_shared_conflict />
                    <section class="mcp-settings" aria-label=move || t("MCP 本地访问")>
                        <div class="mcp-heading">
                            <div>
                                <strong>{move || t("MCP 本地访问")}</strong>
                                <span>{move || t("让明确授权的 AI 客户端通过本机 stdio 搜索、读取和整理历史。默认关闭，不开放网络端口。")}</span>
                            </div>
                            <input
                                aria-label=move || t("启用 MCP 本地访问")
                                type="checkbox"
                                prop:checked=move || mcp_status.get().is_some_and(|value| value.enabled)
                                on:change=toggle_mcp
                            />
                        </div>
                        <div class="mcp-create-row">
                            <input
                                type="text"
                                maxlength="80"
                                placeholder=move || t("客户端名称，例如 Codex")
                                prop:value=move || mcp_client_name.get()
                                on:input=move |event| set_mcp_client_name.set(event_target_value(&event))
                            />
                            <button type="button" on:click=create_mcp_connection>{move || t("创建并启用")}</button>
                        </div>
                        <div class="mcp-clients">
                            {move || match mcp_status.get() {
                                Some(current) if current.clients.is_empty() => view! {
                                    <span class="mcp-empty">{move || t("尚未授权客户端")}</span>
                                }.into_any(),
                                Some(current) => current.clients.into_iter().map(|client| {
                                    let client_id = client.id.clone();
                                    let last_used = client.last_used_at
                                        .as_deref()
                                        .unwrap_or(t("尚未使用"))
                                        .to_owned();
                                    view! {
                                        <div class="mcp-client-row" title=localized_format!("创建于 {} · 最后使用 {}", "Created {} · Last used {}", client.created_at, last_used)>
                                            <span>{client.display_name}</span>
                                            <button
                                                type="button"
                                                on:click=move |_| revoke_mcp_connection.run(client_id.clone())
                                            >{move || t("撤销客户端")}</button>
                                        </div>
                                    }
                                }).collect_view().into_any(),
                                None => view! {
                                    <span class="mcp-empty">{move || t("正在检查授权状态…")}</span>
                                }.into_any(),
                            }}
                        </div>
                        {move || mcp_connection_config.get().map(|configuration| view! {
                            <div class="mcp-config-once">
                                <strong>{move || t("仅显示一次的连接配置")}</strong>
                                <span>{move || t("其中包含访问凭据。保存到目标客户端后请关闭此面板；不要粘贴到聊天或提交到仓库。")}</span>
                                <textarea readonly prop:value=configuration></textarea>
                            </div>
                        })}
                    </section>
                    </div>
                    </div></div>
                    <footer><span>{move || t("通用与隐私选项修改后保存")}</span><button class="save-settings" type="button" disabled=move || language_saving.get() || settings_saving.get() || appearance_saving.get() on:click=save_settings>{move || t("保存设置")}</button></footer>
                </aside>
            })}

            <Show when=move || preview_visible.get() && !native_rail>
                <PreviewOverlay
                    clip=Signal::derive(move || preview_item.get().expect("visible preview has an item"))
                    ready=Signal::derive(move || workspace_ready.get())
                    preview=Signal::derive(move || preview_open.get().and_then(|id|previews.with(|items|items.get(&id).cloned())))
                    loading=Signal::derive(move || preview_open.get().is_some_and(|id|preview_loading.with(|items|items.contains(&id))))
                    editing=Signal::derive(move || preview_open.get().is_some_and(|id|image_edit_loading.with(|items|items.contains(&id))))
                    recognizing=Signal::derive(move || preview_open.get().is_some_and(|id|ocr_loading.with(|items|items.contains(&id))))
                    on_rotate=Callback::new(move |direction| {if let Some(id)=preview_open.get_untracked(){rotate_preview_image.run((id,direction));}})
                    on_recognize=Callback::new(move |_| {if let Some(id)=preview_open.get_untracked(){recognize_preview_text.run(id);}})
                    on_open_link=Callback::new(move |_| {if let Some(id)=preview_open.get_untracked(){open_preview_link.run(id);}})
                    on_close=Callback::new(move |_|close_preview.run(()))
                />
            </Show>
        </main>
    }
}

#[component]
fn ClipCard(
    clip: Signal<ClipItem>,
    age: Signal<String>,
    source: Signal<paste_domain::SourceApplication>,
    source_icon: Signal<Option<String>>,
    selected: Signal<bool>,
    keyboard_active: Signal<bool>,
    stacked: Signal<bool>,
    number: usize,
    on_select: Callback<(bool, bool)>,
    on_activate: Callback<(ClipId, bool)>,
    on_toggle_stack: Callback<ClipId>,
    on_drag: Callback<ClipId>,
    on_context_menu: Callback<(ClipId, f64, f64)>,
    card_press: RwSignal<Option<CardPress>>,
    trace_gesture: Callback<GestureTrace>,
    drop_position: Signal<Option<bool>>,
    on_locate: Callback<ClipId>,
    locatable: Signal<bool>,
    preview_asset: Signal<Option<PreviewResult>>,
) -> impl IntoView {
    let clip_id = clip.with_untracked(|item| item.id);
    let suppress_drag_click = RwSignal::new(false);
    let finish_press = Callback::new(move |event: ev::PointerEvent| {
        if card_press
            .get_untracked()
            .is_some_and(|press| press.belongs_to(clip_id, event.pointer_id()))
        {
            card_press.set(None);
        }
    });
    let class = move || {
        let mut classes =
            clip.with(|item| format!("clip-card kind-{}", item.content_kind.as_str()));
        if selected.get() {
            classes.push_str(" selected");
        }
        if keyboard_active.get() {
            classes.push_str(" keyboard-active");
        }
        if stacked.get() {
            classes.push_str(" stacked");
        }
        if drop_position.get() == Some(false) {
            classes.push_str(" drop-before");
        }
        if drop_position.get() == Some(true) {
            classes.push_str(" drop-after");
        }
        classes
    };
    let kind = move || clip.with(|item| kind_label(item.content_kind));
    let preview = Memo::new(move |_| {
        clip.with(|item| {
            if item.searchable_text.trim().is_empty() {
                item.title.clone()
            } else {
                item.searchable_text.clone()
            }
        })
    });
    let swatch = Memo::new(move |_| {
        clip.with(|item| crate::card_visual::color_swatch(item.content_kind, &item.searchable_text))
    });
    let summary = Memo::new(move |_| {
        clip.with(|item| {
            crate::card_visual::text_summary(
                item.content_kind,
                &item.searchable_text,
                crate::i18n::language(),
            )
            .unwrap_or_else(|| item.source.display_name.clone())
        })
    });
    let summary_title = move || summary.get();
    let source_label =
        move || localized_format!("来源：{}", "Source: {}", source.get().display_name);
    let failed_source_icon = RwSignal::new(None::<String>);
    let visible_source_icon = Signal::derive(move || {
        source_icon
            .get()
            .filter(|url| failed_source_icon.get().as_ref() != Some(url))
    });
    view! {
        <article
            id=format!("clip-card-{clip_id}")
            role="gridcell"
            tabindex="-1"
            aria-colindex=number.to_string()
            aria-selected=move || selected.get().to_string()
            aria-labelledby=format!("clip-title-{clip_id} clip-kind-{clip_id}")
            aria-describedby=format!("clip-source-{clip_id} clip-summary-{clip_id}")
            class=class
            data-drop-clip=clip_id.to_string()
            draggable="false"
            on:contextmenu=move |event| {
                event.prevent_default();
                event.stop_propagation();
                on_context_menu.run((clip_id, f64::from(event.client_x()), f64::from(event.client_y())));
            }
            on:pointerdown:capture=move |event| {
                trace_gesture.run(gesture_trace::pointer(GesturePhase::CardDown, Some(clip_id), &event));
                if event.ctrl_key() { return; }
                if event.target().and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|element| element.closest("button, a, input").ok().flatten().is_some()) {
                    return;
                }
                let Some(press) = CardPress::begin(clip_id, event.pointer_id(), event.client_x(), event.client_y(), event.button(), event.is_primary()) else { return; };
                // Capture must belong to this article, not the delegated window.
                // Prevent WebKit text selection from taking over the gesture.
                event.prevent_default();
                suppress_drag_click.set(false);
                card_press.set(Some(press));
                trace_gesture.run(gesture_trace::pointer(GesturePhase::Armed, Some(clip_id), &event));
                let captured = event.current_target().and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|element| element.set_pointer_capture(event.pointer_id()).is_ok());
                if !captured {
                    trace_gesture.run(gesture_trace::pointer(GesturePhase::CaptureFailed, Some(clip_id), &event));
                }
            }
            on:pointermove:capture=move |event| {
                if event.buttons() != 0 || card_press.get_untracked().is_some() {
                    trace_gesture.run(gesture_trace::pointer(GesturePhase::CardMove, Some(clip_id), &event));
                }
                let Some(press) = card_press.get_untracked().filter(|press| press.belongs_to(clip_id, event.pointer_id())) else { return; };
                if event.buttons() & 1 == 0 { card_press.set(None); return; }
                if !press.ready(event.pointer_id(), event.client_x(), event.client_y(), event.buttons()) { return; }
                trace_gesture.run(gesture_trace::pointer(GesturePhase::Threshold, Some(clip_id), &event));
                event.prevent_default();
                card_press.set(None);
                suppress_drag_click.set(true);
                if let Some(element) = event.current_target().and_then(|target| target.dyn_into::<web_sys::Element>().ok()) {
                    let _ = element.release_pointer_capture(event.pointer_id());
                }
                on_drag.run(clip_id);
            }
            on:pointerup:capture=move |event| { trace_gesture.run(gesture_trace::pointer(GesturePhase::CardUp, Some(clip_id), &event)); finish_press.run(event); }
            on:pointercancel:capture=move |event| { trace_gesture.run(gesture_trace::pointer(GesturePhase::CardCancel, Some(clip_id), &event)); finish_press.run(event); }
            on:lostpointercapture:capture=move |event| { trace_gesture.run(gesture_trace::pointer(GesturePhase::CardLostCapture, Some(clip_id), &event)); finish_press.run(event); }
            on:click=move |event| {
                if suppress_drag_click.get_untracked() { suppress_drag_click.set(false); event.prevent_default(); return; }
                on_select.run((event.meta_key(), event.shift_key()));
            }
            on:dblclick=move |_| on_activate.run((clip_id, false))
            on:dragstart=move |event| {
                event.prevent_default();
            }
        >
            <header class="card-header">
                <div class="card-heading">
                    <strong class="card-kind" id=format!("clip-kind-{clip_id}")>{kind}</strong>
                </div>
                <span class="card-time">{move || age.get()}</span>
            </header>
            <div class="card-content">
                <strong class="card-title" id=format!("clip-title-{clip_id}") title=move || clip.with(|item| item.title.clone())>{move || clip.with(|item| item.title.clone())}</strong>
                {move || if let Some((color, ink)) = swatch.get() {
                    view! { <p class="card-swatch" style=format!("background-color:{color};color:{ink}")>{color.clone()}</p> }.into_any()
                } else {
                    preview_asset.get().map_or_else(
                        || view! { <p class="card-preview">{move || preview.get()}</p> }.into_any(),
                        |asset| view! {
                            <img class="card-media" src=asset.data_url alt=move || if clip.with(|item| item.content_kind == ContentKind::Pdf) { t("PDF 首页缩略图") } else { t("剪贴板图片预览") } loading="lazy" draggable="false" />
                        }.into_any(),
                    )
                }}
            </div>
            <footer>
                <span class="source-badge" id=format!("clip-source-{clip_id}") class:has-icon=move || visible_source_icon.get().is_some() title=source_label aria-label=source_label>
                    {move || visible_source_icon.get().map_or_else(
                        || app_initial(&source.get().display_name).into_any(),
                        |url| view! { <img class="source-app-icon" src=url alt="" draggable="false" on:error=move |_| failed_source_icon.set(source_icon.get_untracked()) /> }.into_any(),
                    )}
                </span>
                <div class="card-provenance">
                    <span class="card-source-name">{move || source.get().display_name}</span>
                <span class="card-summary" id=format!("clip-summary-{clip_id}") title=summary_title>{move || preview_asset.get().filter(|asset| asset.pixel_width > 0 && asset.pixel_height > 0).map_or_else(|| summary.get(), |asset| if clip.with(|item| item.content_kind == ContentKind::Pdf) { t("PDF · 首页").to_owned() } else { format!("{} × {}", asset.pixel_width, asset.pixel_height) })}</span>
                </div>
                <div class="card-actions">
                {move || (number <= 9).then(|| view! {
                    <kbd>{format!("⌘{number}")}</kbd>
                })}
                {move || locatable.get().then(|| view! {
                    <button
                        class="locate-button"
                        type="button"
                        tabindex="-1"
                        aria-label=move || clip.with(|item| localized_format!("跳回历史位置：{}", "Show in history: {}", item.title))
                        title=move || t("跳回历史位置")
                        on:click=move |event| {
                            event.stop_propagation();
                            on_locate.run(clip_id);
                        }
                    >"↗"</button>
                })}
                <button
                    class="stack-toggle"
                    class:active=move || stacked.get()
                    type="button"
                    tabindex="-1"
                    aria-label=move || clip.with(|item| localized_format!("加入或移出 顺序粘贴：{}", "Add to or remove from paste queue: {}", item.title))
                    aria-pressed=move || stacked.get().to_string()
                    title=move || t("加入或移出 顺序粘贴（⌘↩）")
                    on:click=move |event| {
                        event.stop_propagation();
                        on_toggle_stack.run(clip_id);
                    }
                >"＋"</button>
                </div>
            </footer>
        </article>
    }
}

#[component]
fn PdfDocumentPreview(clip_id: ClipId) -> impl IntoView {
    // Full document is fetched only while Quick Look is open, and released
    // with the component. A first-page raster must never stand in for it.
    let result = RwSignal::new(None::<Result<PreviewResult, String>>);
    let pending = RwSignal::new(false);
    let load = Callback::new(move |()| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        result.set(None);
        spawn_local(async move {
            let response = invoke::<PreviewResult>(
                "get_clip_preview",
                &CommandArgs {
                    request: PreviewRequest {
                        clip_id: clip_id.to_string(),
                    },
                },
            )
            .await;
            // The user may have closed/switched the overlay while IPC ran.
            let _ = result.try_set(Some(response));
            let _ = pending.try_set(false);
        });
    });
    load.run(());
    view! {
        {move || match result.get() {
            Some(Ok(asset)) if asset.media_type == "application/pdf" => view! {
                <iframe class="preview-pdf" src=asset.data_url title=move || t("PDF 完整预览")></iframe>
            }.into_any(),
            Some(response) => {
                let message = response.err().unwrap_or_else(|| t("PDF 完整预览不可用。").into());
                view! {
                    <div class="preview-loading" role="status">
                        <p>{move || t(&message).to_owned()}</p>
                        <button type="button" on:click=move |_| load.run(())>{move || t("重试 PDF 预览")}</button>
                    </div>
                }.into_any()
            },
            None => view! { <div class="preview-loading" role="status">{move || t("正在加载完整 PDF…")}</div> }.into_any(),
        }}
    }
}

#[component]
fn PreviewOverlay(
    clip: Signal<ClipItem>,
    ready: Signal<bool>,
    preview: Signal<Option<PreviewResult>>,
    loading: Signal<bool>,
    editing: Signal<bool>,
    recognizing: Signal<bool>,
    on_rotate: Callback<i8>,
    on_recognize: Callback<()>,
    on_open_link: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let preview_root = NodeRef::<html::Aside>::new();
    Effect::new(move |_| {
        if ready.get()
            && let Some(root) = preview_root.get()
        {
            let _ = root.focus();
        }
    });
    let is_image = move || clip.with(|item| item.content_kind == ContentKind::Image);
    let is_link = move || clip.with(|item| item.content_kind == ContentKind::Link);
    let preview_text = move || clip.with(|item| item.searchable_text.clone());
    let media = move || {
        if clip.with(|item| item.content_kind == ContentKind::Pdf) {
            view! { <PdfDocumentPreview clip_id=clip.get().id /> }.into_any()
        } else {
            view! { {move || preview.get().map_or_else(
            || {
                if loading.get() {
                    view! { <div class="preview-loading">{move || t("正在生成预览…")}</div> }.into_any()
                } else {
                    view! { <pre class="preview-text">{preview_text()}</pre> }.into_any()
                }
            },
            |asset| {
                if asset.media_type.starts_with("image/") {
                    let dimensions = format!("{} × {}", asset.pixel_width, asset.pixel_height);
                    view! {
                        <figure class="preview-image">
                            <img src=asset.data_url alt=t("剪贴板图片完整预览") />
                            <figcaption>{dimensions}</figcaption>
                        </figure>
                    }
                    .into_any()
                } else {
                    view! {
                        <iframe
                            class="preview-pdf"
                            src=asset.data_url
                            title=move || t("PDF 预览")
                        ></iframe>
                    }
                    .into_any()
                }
            },
        )} }
        .into_any()
        }
    };
    view! {
        <aside node_ref=preview_root class="preview-overlay" role="dialog" aria-modal="false" tabindex="-1" aria-label=move || t("Quick Look 预览")>
            <header>
                <div>
                    <strong>{move || clip.get().title}</strong>
                    <span>{move || clip.with(|item|format!("{} · {}", kind_label(item.content_kind), item.source.display_name))}</span>
                </div>
                <button type="button" aria-label=move || t("关闭预览") on:click=move |_| on_close.run(())>"×"</button>
            </header>
            <div class="preview-content">{media}</div>
            <footer>
                <span>{move || t("Esc 关闭 · Return 粘贴")}</span>
                {move || is_image().then(|| view! {
                    <div class="preview-actions" aria-label=move || t("图片快速操作")>
                        <button
                            type="button"
                            disabled=move || editing.get() || recognizing.get()
                            title=move || t("向左旋转")
                            on:click=move |_| on_rotate.run(-1)
                        >"↶"</button>
                        <button
                            type="button"
                            disabled=move || editing.get() || recognizing.get()
                            title=move || t("向右旋转")
                            on:click=move |_| on_rotate.run(1)
                        >{move || if editing.get() { t("旋转中…") } else { "↷" }}</button>
                        <button
                            type="button"
                            disabled=move || editing.get() || recognizing.get()
                            on:click=move |_| on_recognize.run(())
                        >{move || if recognizing.get() { t("识别中…") } else { t("提取文字") }}</button>
                    </div>
                })}
                {move || is_link().then(|| view! {
                    <div class="preview-actions">
                        <button
                            type="button"
                            title=move || t("在 CopyRail 内置浏览器中打开（⌘O）")
                            on:click=move |_| on_open_link.run(())
                        >{move || t("内置浏览器打开")}</button>
                    </div>
                })}
            </footer>
        </aside>
    }
}

fn focus_card_button(button: &web_sys::HtmlElement) {
    // Compact hides its footer until focus enters the card. Focusing the
    // cell first reveals visible controls without changing the user's mode
    // or the native window size; focus leaving the card collapses them again.
    if let Ok(Some(card)) = button.closest(".clip-card")
        && let Ok(card) = card.dyn_into::<web_sys::HtmlElement>()
    {
        let _ = card.focus();
    }
    let _ = button.focus();
}

fn is_text_entry_target(event: &ev::KeyboardEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|element| is_text_entry(&element))
}

fn is_text_entry(element: &web_sys::Element) -> bool {
    element
        .closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])")
        .ok()
        .flatten()
        .is_some()
}

fn ordered_selection(
    clips: &[ClipItem],
    selected_ids: &HashSet<ClipId>,
    fallback_index: usize,
) -> Vec<ClipId> {
    let selected = clips
        .iter()
        .filter(|clip| selected_ids.contains(&clip.id))
        .map(|clip| clip.id)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        clips
            .get(fallback_index)
            .map(|clip| vec![clip.id])
            .unwrap_or_default()
    } else {
        selected
    }
}

fn selection_range(clips: &[ClipItem], anchor: usize, end: usize) -> HashSet<ClipId> {
    if clips.is_empty() {
        return HashSet::new();
    }
    let anchor = anchor.min(clips.len() - 1);
    let end = end.min(clips.len() - 1);
    let (start, finish) = if anchor <= end {
        (anchor, end)
    } else {
        (end, anchor)
    };
    clips[start..=finish].iter().map(|clip| clip.id).collect()
}

fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Text => "Text",
        ContentKind::RichText => "Rich Text",
        ContentKind::Html => "HTML",
        ContentKind::Link => "Link",
        ContentKind::Image => "Image",
        ContentKind::File => "File",
        ContentKind::Pdf => "PDF",
        ContentKind::Color => "Color",
        ContentKind::Unknown => "Item",
    }
}

fn is_textually_editable(kind: ContentKind) -> bool {
    matches!(
        kind,
        ContentKind::Text | ContentKind::Link | ContentKind::Color
    )
}

fn needs_rich_text_editor(clip: &ClipItem) -> bool {
    matches!(clip.content_kind, ContentKind::RichText | ContentKind::Html)
        || clip.representations.iter().any(|item| matches!(item.kind, paste_domain::RepresentationKind::Rtf | paste_domain::RepresentationKind::Html)
            || item.native_type.as_deref() == Some("com.apple.flat-rtfd")
            || matches!(&item.kind, paste_domain::RepresentationKind::Custom(kind) if kind == "com.apple.flat-rtfd"))
}

fn app_initial(name: &str) -> String {
    name.chars().next().unwrap_or('•').to_uppercase().collect()
}

fn relative_time(time: chrono::DateTime<chrono::Utc>) -> String {
    let seconds = (chrono::Utc::now() - time).num_seconds().max(0);
    match seconds {
        0..=59 => "now".into(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn relative_timestamp_ms(timestamp_ms: i64) -> String {
    let seconds = ((chrono::Utc::now().timestamp_millis() - timestamp_ms) / 1_000).max(0);
    match seconds {
        0..=59 => t("刚刚").into(),
        60..=3_599 => localized_format!("{} 分钟前", "{}m ago", seconds / 60),
        3_600..=86_399 => localized_format!("{} 小时前", "{}h ago", seconds / 3_600),
        _ => localized_format!("{} 天前", "{}d ago", seconds / 86_400),
    }
}

fn pause_label(status: &CaptureStatus) -> String {
    status.paused_until_ms.map_or_else(
        || t("暂停中 · 点击恢复").into(),
        |until| {
            let remaining_ms = (until - chrono::Utc::now().timestamp_millis()).max(0);
            let remaining_minutes = (remaining_ms + 59_999) / 60_000;
            localized_format!(
                "暂停中 · {remaining_minutes} 分钟 · 点击恢复",
                "Paused · {remaining_minutes}m · Click to resume"
            )
        },
    )
}

fn toggle_stack_item(items: &mut Vec<ClipId>, clip_id: ClipId) {
    if let Some(index) = items.iter().position(|value| *value == clip_id) {
        items.remove(index);
    } else {
        items.push(clip_id);
    }
}

fn quick_paste_index(key: &str) -> Option<usize> {
    let value = key.parse::<usize>().ok()?;
    (1..=9).contains(&value).then_some(value - 1)
}

fn optional_number(value: Option<u32>) -> String {
    value.map_or_else(String::new, |number| number.to_string())
}

fn parse_optional_positive(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok().filter(|value| *value > 0)
}
