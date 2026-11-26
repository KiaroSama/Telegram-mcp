//! Per-request trusted MCP context, tool timeout budget, ordinary-choice
//! elicitation seam, client roots query, and safe refusal wording overrides.
use anyhow::{Context, Result, bail};
use rmcp::{
    Peer, RoleServer,
    model::{
        ElicitRequestParams, ElicitationAction, ElicitationSchema, EnumSchema, ProtocolVersion,
    },
    service::{ElicitationMode, RequestContext},
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, future::Future, path::PathBuf, time::Duration};

pub const DEFAULT_TOOL_TIMEOUT_SECONDS: f64 = 55.0;
pub const DEFAULT_EXPORT_TIMEOUT_SECONDS: f64 = 120.0;
const MAX_FINITE_TIMEOUT_SECONDS: f64 = 86_400.0 * 365.0;
const MAX_REFUSAL_TEXT_CHARS: usize = 512;

/// Parse `TELEGRAM_TOOL_TIMEOUT_SECONDS`:
/// - Omitted, empty, malformed, or non-finite => default (55.0s, or 120.0s for exports)
/// - Finite `<= 0.0` => `None` (disables ONLY the ordinary tool call budget)
/// - Finite `> 0.0` => `Some(Duration)`
pub fn parse_tool_timeout(raw: Option<&str>, default_seconds: f64) -> Option<Duration> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(Duration::from_secs_f64(default_seconds));
    };
    let Ok(seconds) = text.parse::<f64>() else {
        return Some(Duration::from_secs_f64(default_seconds));
    };
    if !seconds.is_finite() {
        return Some(Duration::from_secs_f64(default_seconds));
    }
    if seconds <= 0.0 {
        return None;
    }
    if seconds > MAX_FINITE_TIMEOUT_SECONDS {
        return Some(Duration::from_secs_f64(default_seconds));
    }
    Some(Duration::from_secs_f64(seconds))
}

pub fn tool_budget(name: &str) -> Option<Duration> {
    let default_seconds = if name.starts_with("export_") {
        DEFAULT_EXPORT_TIMEOUT_SECONDS
    } else {
        DEFAULT_TOOL_TIMEOUT_SECONDS
    };
    let raw = std::env::var("TELEGRAM_TOOL_TIMEOUT_SECONDS").ok();
    parse_tool_timeout(raw.as_deref(), default_seconds)
}

pub async fn with_optional_deadline<F, T>(
    deadline: Option<tokio::time::Instant>,
    fut: F,
    timeout_message: &'static str,
) -> Result<T>
where
    F: Future<Output = T>,
{
    match deadline {
        Some(limit) => tokio::time::timeout_at(limit, fut)
            .await
            .map_err(|_| anyhow::anyhow!(timeout_message)),
        None => Ok(fut.await),
    }
}

/// Optional safe presentation overrides loaded from `TELEGRAM_REFUSALS` and
/// `TELEGRAM_REFUSAL_CODES`. Applied strictly AFTER a denial/error decision has
/// already been made; never turns failure into success, never injects HTML, and
/// never exposes raw error details.
#[derive(Debug, Clone, Default)]
pub struct RefusalOverrides {
    by_name: BTreeMap<String, String>,
    by_code: BTreeMap<u16, String>,
}

fn is_recognized_error_key(key: &str) -> bool {
    (2..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && key.bytes().any(|b| b.is_ascii_uppercase())
}

fn sanitize_refusal_message(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_REFUSAL_TEXT_CHARS {
        return None;
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || c == '<' || c == '>')
    {
        return None;
    }
    Some(trimmed.to_owned())
}

impl RefusalOverrides {
    pub fn from_environment() -> Self {
        let names = std::env::var("TELEGRAM_REFUSALS").ok();
        let codes = std::env::var("TELEGRAM_REFUSAL_CODES").ok();
        Self::parse(names.as_deref(), codes.as_deref())
    }

    pub fn parse(raw_names: Option<&str>, raw_codes: Option<&str>) -> Self {
        let mut overrides = Self::default();
        if let Some(raw) = raw_names.map(str::trim).filter(|s| !s.is_empty()) {
            match serde_json::from_str::<Value>(raw) {
                Ok(Value::Object(map)) => {
                    for (key, val) in map {
                        let Some(text) = val.as_str().and_then(sanitize_refusal_message) else {
                            tracing::warn!(
                                "TELEGRAM_REFUSALS: entry value failed plain-text safety bounds"
                            );
                            continue;
                        };
                        if !is_recognized_error_key(&key) {
                            tracing::warn!("TELEGRAM_REFUSALS: unrecognized error key ignored");
                            continue;
                        }
                        overrides.by_name.insert(key, text);
                    }
                }
                _ => tracing::warn!("TELEGRAM_REFUSALS: must be a UTF-8 JSON object"),
            }
        }
        if let Some(raw) = raw_codes.map(str::trim).filter(|s| !s.is_empty()) {
            match serde_json::from_str::<Value>(raw) {
                Ok(Value::Object(map)) => {
                    for (key, val) in map {
                        let Some(text) = val.as_str().and_then(sanitize_refusal_message) else {
                            tracing::warn!(
                                "TELEGRAM_REFUSAL_CODES: entry value failed plain-text safety bounds"
                            );
                            continue;
                        };
                        let parsed = if key.bytes().all(|b| b.is_ascii_digit()) {
                            key.parse::<u16>()
                                .ok()
                                .filter(|code| (100..=599).contains(code))
                        } else {
                            None
                        };
                        match parsed {
                            Some(code) => {
                                overrides.by_code.insert(code, text);
                            }
                            None => tracing::warn!(
                                "TELEGRAM_REFUSAL_CODES: key must be a decimal Telegram status code in 100..=599"
                            ),
                        }
                    }
                }
                _ => tracing::warn!("TELEGRAM_REFUSAL_CODES: must be a UTF-8 JSON object"),
            }
        }
        overrides
    }

    pub fn render(&self, error_name: Option<&str>, code: Option<u16>, fallback: &str) -> String {
        if let Some(name) = error_name
            && let Some(custom) = self.by_name.get(name)
        {
            return custom.clone();
        }
        if let Some(code) = code
            && let Some(custom) = self.by_code.get(&code)
        {
            return custom.clone();
        }
        fallback.to_owned()
    }

    pub fn render_error_text(&self, error_text: &str) -> String {
        if self.by_name.is_empty() && self.by_code.is_empty() {
            return error_text.to_owned();
        }
        let detected_name = self
            .by_name
            .keys()
            .find(|key| error_text.contains(key.as_str()))
            .map(String::as_str);
        let detected_code = self
            .by_code
            .keys()
            .find(|code| error_text.contains(&code.to_string()))
            .copied();
        self.render(detected_name, detected_code, error_text)
    }
}

/// Trusted per-request MCP context. Never accepts caller-supplied approval tokens
/// or treats an ordinary-choice elicitation response as human mutation approval.
#[derive(Clone)]
pub struct TrustedRequestContext {
    pub request_id: String,
    pub protocol_version: Option<String>,
    pub supports_form_elicitation: bool,
    pub supports_roots: bool,
    peer: Option<Peer<RoleServer>>,
    trusted_dialog: bool,
}

impl TrustedRequestContext {
    pub fn local() -> Self {
        Self {
            request_id: format!("local-{:016x}", rand::random::<u64>()),
            protocol_version: None,
            supports_form_elicitation: false,
            supports_roots: false,
            peer: None,
            trusted_dialog: false,
        }
    }

    pub fn from_mcp(context: &RequestContext<RoleServer>) -> Self {
        let peer_info = context.peer.peer_info();
        let protocol_version = context
            .protocol_version()
            .map(|version| version.to_string());
        let supports_form_elicitation = context
            .peer
            .supported_elicitation_modes()
            .contains(&ElicitationMode::Form);
        let supports_roots = peer_info.is_some_and(|info| info.capabilities.roots.is_some());
        Self {
            request_id: Self::local().request_id,
            protocol_version,
            supports_form_elicitation,
            supports_roots,
            peer: Some(context.peer.clone()),
            trusted_dialog: context
                .extensions
                .get::<axum::http::request::Parts>()
                .is_none(),
        }
    }

    pub(super) fn dialog_owner(&self) -> Option<i64> {
        if !self.trusted_dialog || self.peer.is_none() {
            return None;
        }
        // The local owner opts in to a client that presents approval forms to a human.
        std::env::var("TELEGRAM_APPROVAL_TRUSTED_DIALOG_OWNER_ID")
            .ok()?
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
    }

    pub(super) fn peer(&self) -> Option<&Peer<RoleServer>> {
        self.peer.as_ref()
    }

    /// True only when the client supports form elicitation AND negotiated a
    /// protocol version strictly older than `2026-07-28`.
    pub fn supports_ordinary_elicitation(&self) -> bool {
        let Some(version) = self.protocol_version.as_deref() else {
            return false;
        };
        self.supports_form_elicitation && version < ProtocolVersion::V_2026_07_28.as_str()
    }

    /// Query client-advertised MCP roots for this request.
    /// Returns `None` when unsupported or unavailable, distinct from `Some(vec![])`.
    #[allow(deprecated)]
    pub async fn fetch_client_roots(&self) -> Option<Vec<PathBuf>> {
        if !self.supports_roots {
            return None;
        }
        let peer = self.peer.as_ref()?;
        let response = tokio::time::timeout(Duration::from_secs(3), peer.list_roots())
            .await
            .ok()?
            .ok()?;
        let mut paths = Vec::new();
        for root in response.roots {
            if let Ok(url) = url::Url::parse(&root.uri)
                && url.scheme() == "file"
                && let Ok(path) = url.to_file_path()
            {
                paths.push(path);
            }
        }
        Some(paths)
    }

    /// Ordinary-choice MCP elicitation seam for `export_secret_chat` called BEFORE
    /// schema defaults are applied. Never grants human mutation approval and never
    /// silently exports on cancellation or unsupported clients.
    pub async fn elicit_missing_export_choices(
        &self,
        raw_args: &mut Value,
    ) -> Result<Option<Value>> {
        let missing = crate::tools::secret::autosave::missing_export_choices(raw_args);
        if missing.is_empty() {
            return Ok(None);
        }
        if !self.supports_ordinary_elicitation() {
            return Ok(None);
        }
        let Some(peer) = self.peer.as_ref() else {
            return Ok(None);
        };
        let schema = build_export_choice_schema(&missing)?;
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Choose the export options for this Secret Chat export.".to_owned(),
            requested_schema: schema,
        };
        let outcome = match peer
            .create_elicitation_with_timeout(params, Some(Duration::from_secs(120)))
            .await
        {
            Ok(result) => result,
            Err(_) => return Ok(None),
        };
        match outcome.action {
            ElicitationAction::Accept => {
                if let Some(content) = outcome.content.and_then(|v| v.as_object().cloned()) {
                    let target = raw_args
                        .as_object_mut()
                        .context("Tool arguments must be an object")?;
                    merge_elicited_choices(target, &missing, &content);
                }
                Ok(None)
            }
            ElicitationAction::Cancel | ElicitationAction::Decline | _ => Ok(Some(json!({
                "status": "cancelled",
                "performed": false,
                "required": missing,
            }))),
        }
    }
}

pub fn merge_elicited_choices(
    target: &mut Map<String, Value>,
    missing: &[&str],
    elicited: &Map<String, Value>,
) {
    for &field in missing {
        if let Some(value) = elicited.get(field)
            && !value.is_null()
        {
            target.insert(field.to_owned(), value.clone());
        }
    }
}

pub fn build_export_choice_schema(missing: &[&str]) -> Result<ElicitationSchema> {
    let mut builder = ElicitationSchema::builder().title("Secret Chat export options");
    for &field in missing {
        builder = match field {
            "format" => builder.required_enum_schema(
                "format",
                EnumSchema::builder(vec![
                    "html".to_owned(),
                    "json".to_owned(),
                    "html_and_json".to_owned(),
                ])
                .title("Export format")
                .build(),
            ),
            "photos" | "videos" | "voice_messages" | "video_messages" | "stickers" | "gifs"
            | "files" => builder.optional_bool(field, false),
            "size_limit_mb" => builder.optional_number("size_limit_mb", 1.0, 4000.0),
            "date_from" | "date_to" | "destination" => builder.optional_string(field),
            _ => builder,
        };
    }
    builder
        .build()
        .map_err(|reason| anyhow::anyhow!("Invalid elicitation schema: {reason}"))
}

/// Tool-specific preflight checks that must run BEFORE Ghost Mode or Telegram peer I/O.
pub fn preflight_validate_tool(name: &str, args: &Value) -> Result<()> {
    if name == "send_typing" {
        crate::tools::messaging::helpers::validate_typing(args)?;
    }
    if name == "accept_secret_chat" {
        let raw = args
            .get("chat_id")
            .context("accept_secret_chat requires chat_id")?;
        if super::recipient::wire_secret_chat_id(raw).is_none() {
            bail!("chat_id must identify a non-zero signed 32-bit secret chat");
        }
    }
    Ok(())
}
