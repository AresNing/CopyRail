#![forbid(unsafe_code)]

use std::{str::FromStr, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use paste_domain::{
    ClipId, ContentKind, PinboardId, SearchFilters, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::{SqliteStore, StorageError};
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::wrapper::{Json, Parameters},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_SEARCH_RESULTS: u32 = 50;
const MAX_TEXT_REPRESENTATION_BYTES: usize = 1024 * 1024;
const MAX_BINARY_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const PREVIEW_CHARACTERS: usize = 1_000;

#[derive(Clone)]
pub struct PasteMcpServer {
    store: Arc<SqliteStore>,
    token_hash: [u8; 32],
}

impl PasteMcpServer {
    pub fn new(store: Arc<SqliteStore>, token_hash: [u8; 32]) -> Self {
        Self { store, token_hash }
    }

    fn ensure_authorized(&self) -> Result<(), ErrorData> {
        if self
            .store
            .authorize_mcp_token(&self.token_hash)
            .map_err(storage_error)?
            .is_some()
        {
            Ok(())
        } else {
            Err(ErrorData::invalid_request(
                "MCP access is disabled or this client was revoked",
                None,
            ))
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchParams {
    #[serde(default)]
    query: String,
    #[serde(default)]
    content_kinds: Vec<String>,
    pinboard_id: Option<String>,
    copied_within_days: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClipParams {
    clip_id: String,
    #[serde(default)]
    include_binary: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PinItemsParams {
    pinboard_id: String,
    clip_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreatePinboardParams {
    name: String,
    color: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateTextParams {
    text: String,
    pinboard_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RenameItemParams {
    clip_id: String,
    title: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ClipSummary {
    id: String,
    title: String,
    content_kind: String,
    source_application: String,
    source_bundle_id: String,
    device: String,
    copied_at: String,
    text_preview: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SearchOutput {
    items: Vec<ClipSummary>,
    returned: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct RepresentationOutput {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_type: Option<String>,
    mime_type: Option<String>,
    file_name: Option<String>,
    byte_len: usize,
    text: Option<String>,
    base64: Option<String>,
    omitted_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ItemOutput {
    item: ClipSummary,
    representations: Vec<RepresentationOutput>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct PinboardOutput {
    id: String,
    name: String,
    color: String,
    item_count: u64,
    is_shared: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct PinboardsOutput {
    pinboards: Vec<PinboardOutput>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct MutationOutput {
    success: bool,
    affected_items: usize,
    id: Option<String>,
}

#[tool_router]
impl PasteMcpServer {
    #[tool(
        name = "search_history",
        description = "Search approved CopyRail clipboard history and OCR text. Supports content kind, Pinboard, recent-day, and bounded result filters."
    )]
    fn search_history(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<SearchOutput>, ErrorData> {
        self.ensure_authorized()?;
        let limit = params.limit.unwrap_or(20);
        if !(1..=MAX_SEARCH_RESULTS).contains(&limit) {
            return Err(ErrorData::invalid_params(
                "limit must be between 1 and 50",
                None,
            ));
        }
        let content_kinds = params
            .content_kinds
            .iter()
            .map(|value| {
                ContentKind::from_str(value).map_err(|_| {
                    ErrorData::invalid_params(format!("unsupported content kind: {value}"), None)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let pinboard_ids = params
            .pinboard_id
            .map(|value| parse_pinboard_id(&value))
            .transpose()?
            .into_iter()
            .collect();
        let copied_after = params
            .copied_within_days
            .map(|days| Utc::now() - Duration::days(i64::from(days.clamp(1, 36_500))));
        let hits = self
            .store
            .search_items(
                &SearchQuery {
                    text: params.query,
                    filters: SearchFilters {
                        content_kinds,
                        pinboard_ids,
                        copied_after,
                        ..SearchFilters::default()
                    },
                },
                SearchPage::new(limit, 0),
            )
            .map_err(storage_error)?;
        let items = hits
            .into_iter()
            .map(|item| clip_summary(&item))
            .collect::<Vec<_>>();
        Ok(Json(SearchOutput {
            returned: items.len(),
            items,
        }))
    }

    #[tool(
        name = "get_item",
        description = "Read one approved CopyRail item by ID. Text is bounded to 1 MiB per representation; binary data is omitted unless explicitly requested and is capped at 5 MiB total."
    )]
    fn get_item(
        &self,
        Parameters(params): Parameters<ClipParams>,
    ) -> Result<Json<ItemOutput>, ErrorData> {
        self.ensure_authorized()?;
        let clip_id = parse_clip_id(&params.clip_id)?;
        let item = self
            .store
            .get_clip(clip_id)
            .map_err(storage_error)?
            .ok_or_else(|| ErrorData::resource_not_found("clipboard item not found", None))?;
        let payload = self
            .store
            .load_clipboard_payload(clip_id)
            .map_err(storage_error)?;
        let mut binary_budget = MAX_BINARY_RESPONSE_BYTES;
        let representations = payload
            .into_iter()
            .map(|representation| {
                let byte_len = representation.bytes.len();
                let text = if byte_len <= MAX_TEXT_REPRESENTATION_BYTES * 2 {
                    representation.decoded_text().and_then(|value| {
                        (value.len() <= MAX_TEXT_REPRESENTATION_BYTES).then(|| value.into_owned())
                    })
                } else {
                    None
                };
                let (base64, omitted_reason) = if text.is_some() {
                    (None, None)
                } else if !params.include_binary {
                    (None, Some("binary output was not requested".into()))
                } else if byte_len > binary_budget {
                    (
                        None,
                        Some("binary output exceeds the remaining 5 MiB response budget".into()),
                    )
                } else {
                    binary_budget -= byte_len;
                    (Some(STANDARD.encode(&representation.bytes)), None)
                };
                RepresentationOutput {
                    native_type: representation.native_type,
                    kind: representation.kind.storage_key(),
                    mime_type: representation.mime_type,
                    file_name: representation.file_name,
                    byte_len,
                    text,
                    base64,
                    omitted_reason,
                }
            })
            .collect();
        Ok(Json(ItemOutput {
            item: clip_summary(&item),
            representations,
        }))
    }

    #[tool(
        name = "list_pinboards",
        description = "List approved CopyRail Pinboards and item counts."
    )]
    fn list_pinboards(&self) -> Result<Json<PinboardsOutput>, ErrorData> {
        self.ensure_authorized()?;
        let pinboards = self
            .store
            .list_pinboards()
            .map_err(storage_error)?
            .into_iter()
            .map(|pinboard| PinboardOutput {
                id: pinboard.id.to_string(),
                name: pinboard.name,
                color: pinboard.color,
                item_count: pinboard.item_count,
                is_shared: pinboard.is_shared,
            })
            .collect();
        Ok(Json(PinboardsOutput { pinboards }))
    }

    #[tool(
        name = "create_pinboard",
        description = "Create a local CopyRail Pinboard with a #RRGGBB color."
    )]
    fn create_pinboard(
        &self,
        Parameters(params): Parameters<CreatePinboardParams>,
    ) -> Result<Json<MutationOutput>, ErrorData> {
        self.ensure_authorized()?;
        let pinboard = self
            .store
            .create_pinboard(&params.name, &params.color)
            .map_err(storage_error)?;
        Ok(Json(MutationOutput {
            success: true,
            affected_items: 0,
            id: Some(pinboard.id.to_string()),
        }))
    }

    #[tool(
        name = "pin_items",
        description = "Add one or more existing CopyRail item IDs to a Pinboard."
    )]
    fn pin_items(
        &self,
        Parameters(params): Parameters<PinItemsParams>,
    ) -> Result<Json<MutationOutput>, ErrorData> {
        self.ensure_authorized()?;
        let pinboard_id = parse_pinboard_id(&params.pinboard_id)?;
        let clip_ids = parse_clip_ids(&params.clip_ids)?;
        self.store
            .pin_clips(pinboard_id, &clip_ids)
            .map_err(storage_error)?;
        Ok(Json(MutationOutput {
            success: true,
            affected_items: clip_ids.len(),
            id: Some(pinboard_id.to_string()),
        }))
    }

    #[tool(
        name = "unpin_items",
        description = "Remove one or more existing CopyRail item IDs from a Pinboard without deleting history."
    )]
    fn unpin_items(
        &self,
        Parameters(params): Parameters<PinItemsParams>,
    ) -> Result<Json<MutationOutput>, ErrorData> {
        self.ensure_authorized()?;
        let pinboard_id = parse_pinboard_id(&params.pinboard_id)?;
        let clip_ids = parse_clip_ids(&params.clip_ids)?;
        self.store
            .unpin_clips(pinboard_id, &clip_ids)
            .map_err(storage_error)?;
        Ok(Json(MutationOutput {
            success: true,
            affected_items: clip_ids.len(),
            id: Some(pinboard_id.to_string()),
        }))
    }

    #[tool(
        name = "create_text_item",
        description = "Create a local plain-text CopyRail item and optionally pin it to a Pinboard."
    )]
    fn create_text_item(
        &self,
        Parameters(params): Parameters<CreateTextParams>,
    ) -> Result<Json<MutationOutput>, ErrorData> {
        self.ensure_authorized()?;
        let pinboard_id = params
            .pinboard_id
            .map(|value| parse_pinboard_id(&value))
            .transpose()?;
        let device = self
            .store
            .get_or_create_device("This Mac")
            .map_err(storage_error)?;
        let item = self
            .store
            .create_textual_item_in_pinboard(
                ContentKind::Text,
                &params.text,
                SourceApplication {
                    bundle_identifier: "io.pasters.mcp".into(),
                    display_name: "CopyRail MCP".into(),
                },
                device,
                pinboard_id,
            )
            .map_err(storage_error)?;
        Ok(Json(MutationOutput {
            success: true,
            affected_items: 1,
            id: Some(item.id.to_string()),
        }))
    }

    #[tool(
        name = "rename_item",
        description = "Rename one CopyRail item without altering its clipboard payload."
    )]
    fn rename_item(
        &self,
        Parameters(params): Parameters<RenameItemParams>,
    ) -> Result<Json<MutationOutput>, ErrorData> {
        self.ensure_authorized()?;
        let clip_id = parse_clip_id(&params.clip_id)?;
        self.store
            .rename_clip(clip_id, &params.title)
            .map_err(storage_error)?;
        Ok(Json(MutationOutput {
            success: true,
            affected_items: 1,
            id: Some(clip_id.to_string()),
        }))
    }
}

#[tool_handler(
    name = "pasters-mcp",
    version = "0.1.0",
    instructions = "Local approved access to CopyRail clipboard history and Pinboards. Search before reading full items; request binary output only when necessary."
)]
impl ServerHandler for PasteMcpServer {}

#[derive(Debug, Error)]
pub enum McpStartupError {
    #[error("PASTERS_DATABASE_PATH is required")]
    MissingDatabasePath,
    #[error("PASTERS_MCP_TOKEN is required")]
    MissingAccessToken,
    #[error("MCP access is disabled or this client token was revoked")]
    Unauthorized,
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Initialize(Box<rmcp::service::ServerInitializeError>),
    #[error(transparent)]
    ServiceTask(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn run_stdio() -> Result<(), McpStartupError> {
    let database_path =
        std::env::var_os("PASTERS_DATABASE_PATH").ok_or(McpStartupError::MissingDatabasePath)?;
    let token =
        std::env::var("PASTERS_MCP_TOKEN").map_err(|_| McpStartupError::MissingAccessToken)?;
    let token_hash = *blake3::hash(token.as_bytes()).as_bytes();
    let store = Arc::new(SqliteStore::open(database_path)?);
    if store.authorize_mcp_token(&token_hash)?.is_none() {
        return Err(McpStartupError::Unauthorized);
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let service = PasteMcpServer::new(store, token_hash)
            .serve(stdio())
            .await
            .map_err(|error| McpStartupError::Initialize(Box::new(error)))?;
        service.waiting().await?;
        Ok(())
    })
}

fn clip_summary(item: &paste_domain::ClipItem) -> ClipSummary {
    ClipSummary {
        id: item.id.to_string(),
        title: item.title.clone(),
        content_kind: item.content_kind.as_str().to_owned(),
        source_application: item.source.display_name.clone(),
        source_bundle_id: item.source.bundle_identifier.clone(),
        device: item.device.display_name.clone(),
        copied_at: item.last_copied_at.to_rfc3339(),
        text_preview: truncate_chars(&item.searchable_text, PREVIEW_CHARACTERS),
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let result = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{result}…")
    } else {
        result
    }
}

fn parse_clip_id(value: &str) -> Result<ClipId, ErrorData> {
    ClipId::from_str(value).map_err(|_| ErrorData::invalid_params("invalid clip_id", None))
}

fn parse_pinboard_id(value: &str) -> Result<PinboardId, ErrorData> {
    PinboardId::from_str(value).map_err(|_| ErrorData::invalid_params("invalid pinboard_id", None))
}

fn parse_clip_ids(values: &[String]) -> Result<Vec<ClipId>, ErrorData> {
    if values.is_empty() || values.len() > 200 {
        return Err(ErrorData::invalid_params(
            "clip_ids must contain between 1 and 200 items",
            None,
        ));
    }
    values.iter().map(|value| parse_clip_id(value)).collect()
}

fn storage_error(error: StorageError) -> ErrorData {
    match error {
        StorageError::NotFound => ErrorData::resource_not_found("CopyRail item not found", None),
        StorageError::InvalidPinboardName
        | StorageError::InvalidPinboardColor
        | StorageError::EmptyTextualEdit
        | StorageError::TextualEditTooLarge => ErrorData::invalid_params(error.to_string(), None),
        _ => ErrorData::internal_error("CopyRail storage operation failed", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paste_domain::{CaptureFlags, CapturedItem, CapturedRepresentation, DeviceMetadata};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn test_server() -> PasteMcpServer {
        let store = Arc::new(SqliteStore::open_in_memory().expect("open database"));
        let token_hash = *blake3::hash(b"test MCP token").as_bytes();
        store
            .register_mcp_client("Test client", &token_hash)
            .expect("register client");
        store.set_mcp_enabled(true).expect("enable MCP");
        store
            .insert_capture(&CapturedItem {
                captured_at: Utc::now(),
                source: SourceApplication {
                    bundle_identifier: "com.example.notes".into(),
                    display_name: "Notes".into(),
                },
                device: DeviceMetadata {
                    id: paste_domain::DeviceId::new(),
                    display_name: "Test Mac".into(),
                },
                flags: CaptureFlags::default(),
                representations: vec![CapturedRepresentation::plain_text(
                    "bounded MCP release checklist",
                )],
            })
            .expect("insert fixture");
        PasteMcpServer::new(store, token_hash)
    }

    #[test]
    fn utf16_text_is_decoded_but_raw_type_and_response_limits_are_preserved() {
        let server = test_server();
        let text = "合成内容 🦀";
        let large_text = "中".repeat(MAX_TEXT_REPRESENTATION_BYTES / 3 + 1);
        let representations = [text, &large_text]
            .into_iter()
            .map(|text| CapturedRepresentation {
                native_type: Some("public.utf16-external-plain-text".into()),
                bytes: text.encode_utf16().flat_map(u16::to_ne_bytes).collect(),
                ..CapturedRepresentation::plain_text("")
            })
            .collect::<Vec<_>>();
        let item = server
            .store
            .insert_capture(&CapturedItem {
                captured_at: Utc::now(),
                source: SourceApplication::unknown(),
                device: DeviceMetadata {
                    id: paste_domain::DeviceId::new(),
                    display_name: "Synthetic Mac".into(),
                },
                flags: CaptureFlags::default(),
                representations: representations.clone(),
            })
            .expect("synthetic UTF-16");
        let result = server
            .get_item(Parameters(ClipParams {
                clip_id: item.id.to_string(),
                include_binary: false,
            }))
            .expect("authorized read")
            .0;
        assert_eq!(result.representations[0].text.as_deref(), Some(text));
        assert_eq!(
            result.representations[0].native_type.as_deref(),
            Some("public.utf16-external-plain-text")
        );
        assert_eq!(
            result.representations[0].byte_len,
            representations[0].bytes.len()
        );
        assert!(result.representations[1].text.is_none());
        assert!(result.representations[1].base64.is_none());
        assert!(result.representations[1].omitted_reason.is_some());
        assert_eq!(
            server
                .store
                .load_clipboard_payload(item.id)
                .expect("raw payload"),
            representations
        );
    }

    #[test]
    fn search_is_bounded_and_returns_approved_metadata() {
        let server = test_server();
        let result = server
            .search_history(Parameters(SearchParams {
                query: "release".into(),
                content_kinds: vec!["text".into()],
                pinboard_id: None,
                copied_within_days: Some(1),
                limit: Some(10),
            }))
            .expect("search")
            .0;
        assert_eq!(result.returned, 1);
        assert_eq!(result.items[0].source_application, "Notes");
        assert!(result.items[0].text_preview.contains("checklist"));
        assert!(
            server
                .search_history(Parameters(SearchParams {
                    query: String::new(),
                    content_kinds: Vec::new(),
                    pinboard_id: None,
                    copied_within_days: None,
                    limit: Some(51),
                }))
                .is_err()
        );
    }

    #[test]
    fn binary_payload_requires_explicit_request_and_budget() {
        let server = test_server();
        let clip_id = server
            .store
            .list_history(SearchPage::default())
            .expect("history")[0]
            .id;
        let result = server
            .get_item(Parameters(ClipParams {
                clip_id: clip_id.to_string(),
                include_binary: false,
            }))
            .expect("get text item")
            .0;
        assert_eq!(result.representations.len(), 1);
        assert!(result.representations[0].text.is_some());
        assert!(result.representations[0].base64.is_none());
    }

    #[test]
    fn revocation_stops_an_already_running_service_instance() {
        let server = test_server();
        let client = server.store.list_mcp_clients().expect("list clients")[0].clone();
        server
            .store
            .revoke_mcp_client(client.id)
            .expect("revoke client");
        assert!(
            server
                .search_history(Parameters(SearchParams {
                    query: String::new(),
                    content_kinds: Vec::new(),
                    pinboard_id: None,
                    copied_within_days: None,
                    limit: Some(10),
                }))
                .is_err()
        );
    }

    #[tokio::test]
    async fn json_rpc_handshake_lists_the_complete_bounded_tool_surface() {
        let server = test_server();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_io);
        let server_task = tokio::spawn(async move {
            let service = server
                .serve((server_read, server_write))
                .await
                .expect("initialize service");
            service.waiting().await.expect("server task")
        });
        let (client_read, mut client_write) = tokio::io::split(client_io);
        let mut client_lines = BufReader::new(client_read).lines();

        client_write
            .write_all(
                br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"integration-test","version":"1.0"}}}
"#,
            )
            .await
            .expect("write initialize");
        let initialize: serde_json::Value = serde_json::from_str(
            &client_lines
                .next_line()
                .await
                .expect("read initialize")
                .expect("initialize response"),
        )
        .expect("initialize JSON");
        assert_eq!(initialize["id"], 1);
        assert_eq!(initialize["result"]["serverInfo"]["name"], "pasters-mcp");

        client_write
            .write_all(
                br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
"#,
            )
            .await
            .expect("write tool list");
        let tools: serde_json::Value = serde_json::from_str(
            &client_lines
                .next_line()
                .await
                .expect("read tools")
                .expect("tools response"),
        )
        .expect("tools JSON");
        assert_eq!(tools["id"], 2);
        let mut names = tools["result"]["tools"]
            .as_array()
            .expect("tool array")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                "create_pinboard",
                "create_text_item",
                "get_item",
                "list_pinboards",
                "pin_items",
                "rename_item",
                "search_history",
                "unpin_items",
            ]
        );

        drop(client_write);
        drop(client_lines);
        tokio::time::timeout(std::time::Duration::from_secs(1), server_task)
            .await
            .expect("server should observe transport close")
            .expect("join server task");
    }
}
