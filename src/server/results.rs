//! Safe MCP tool result projection: internal field redaction, durable seal
//! redaction, untrusted Telegram text sanitization, `structuredContent` output
//! schema support, and `audience: ["user"]` content block helpers.
use crate::{
    safeguard::Safeguard,
    sanitize::{SanitizeValueOptions, sanitize_value_with_options},
    state::State,
    text_fidelity::DEFAULT_MAX_TEXT_CHARS,
};
use rmcp::model::{Annotations, CallToolResult, ContentBlock, Role, TextContent, Tool};
use serde_json::{Value, json};
use std::path::PathBuf;

const TRUSTED_METADATA_KEYS: &[&str] = &[
    "_",
    "id",
    "chat_id",
    "user_id",
    "channel_id",
    "secret_chat_id",
    "wire_chat_id",
    "peer_user_id",
    "message_id",
    "first_message_id",
    "last_message_id",
    "reply_to_msg_id",
    "topic_id",
    "account",
    "account_generation",
    "label",
    "status",
    "state",
    "reason",
    "reasons",
    "required",
    "fingerprint",
    "config_key",
    "config_digest",
    "backend",
    "source",
    "format",
    "path",
    "roots",
    "configured_folders",
    "free_folders",
    "always_allowed_folders",
    "client_roots",
    "watch_command",
    "rule",
    "human_action",
    "local_warnings",
    "next_cursor",
    "cursor",
    "link",
    "permalink",
    "url",
    "uri",
    "download_url",
    "upload_url",
    "mime_type",
    "mimeType",
    "type",
    "data",
    "sha256",
    "md5",
    "md5_checksum",
    "date",
    "ts",
    "version",
    "code",
    "error",
];

const SINGLE_LINE_UNTRUSTED_KEYS: &[&str] = &[
    "name",
    "display_name",
    "first_name",
    "last_name",
    "title",
    "username",
    "author",
];

/// Build a text content block annotated with `audience: ["user"]`.
pub fn user_audience_text(text: impl Into<String>) -> ContentBlock {
    let mut annotations = Annotations::default();
    annotations.audience = Some(vec![Role::User]);
    ContentBlock::Text(TextContent::new(text).with_annotations(annotations))
}

/// Remove internal transport credentials and compose durable seal redaction with
/// recursive untrusted Telegram text sanitization. Neither sanitizer order nor
/// formatting can reveal a seal.
pub fn project_tool_result(
    guard: &Safeguard,
    account: &str,
    local_only: bool,
    mut value: Value,
) -> Value {
    redact_internal(&mut value);
    let redacted = guard.redact_tool_result(account, &value);
    if local_only {
        return redacted;
    }
    let options = SanitizeValueOptions {
        max_length: DEFAULT_MAX_TEXT_CHARS,
        trusted_keys: TRUSTED_METADATA_KEYS,
        single_line_keys: SINGLE_LINE_UNTRUSTED_KEYS,
        redactor: None,
    };
    let sanitized = sanitize_value_with_options(redacted, &options);
    // Re-apply seal redaction after projection so normalization cannot reconstruct a seal.
    guard.redact_tool_result(account, &sanitized)
}

pub(super) fn project_tool_error(guard: &Safeguard, message: String) -> CallToolResult {
    let value = guard.redact_tool_result("default", &Value::String(message));
    CallToolResult::error(vec![ContentBlock::text(
        value.as_str().unwrap_or("Tool invocation failed"),
    )])
}

/// Build an MCP `CallToolResult`. When the tool descriptor declares `outputSchema`
/// (e.g. `accept_secret_chat`), publish both text JSON and `structuredContent`.
pub fn build_call_tool_result(tool: Option<&Tool>, mut value: Value) -> CallToolResult {
    let has_output_schema = tool.and_then(|t| t.output_schema.as_ref()).is_some();
    if let Some(blocks) = value
        .as_object_mut()
        .and_then(|object| object.remove("content"))
        .and_then(|value| value.as_array().cloned())
    {
        let content = blocks
            .into_iter()
            .map(serde_json::from_value::<ContentBlock>)
            .collect::<std::result::Result<Vec<_>, _>>();
        if let Ok(mut content) = content {
            let structured = if has_output_schema {
                Some(value.clone())
            } else {
                None
            };
            if value.as_object().is_some_and(|object| !object.is_empty()) {
                content.push(ContentBlock::text(value.to_string()));
            }
            let mut result = CallToolResult::success(content);
            result.structured_content = structured;
            return result;
        }
        return CallToolResult::error(vec![ContentBlock::text(
            "Invalid content returned by a native tool",
        )]);
    }
    let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
    if has_output_schema {
        result.structured_content = Some(value);
    }
    result
}

pub fn roots_status_payload(state: &State, client_roots: Option<Vec<PathBuf>>) -> Value {
    let free = state.free_folders();
    let always = state.always_allowed_folders(None);
    let mut all_roots = free.clone();
    for dir in &always {
        if !all_roots.contains(dir) {
            all_roots.push(dir.clone());
        }
    }
    let client_roots_status = match &client_roots {
        None => "unavailable",
        Some(list) if list.is_empty() => "empty",
        Some(_) => "advertised",
    };
    json!({
        "file_tools_enabled": !all_roots.is_empty(),
        "status": if all_roots.is_empty() { "no_allowed_roots" } else { "configured" },
        "roots": all_roots,
        "configured_folders": free,
        "free_folders": free,
        "always_allowed_folders": always,
        "client_roots": client_roots,
        "client_roots_status": client_roots_status,
        "rule": "File operations stay within configured roots. The private state directory and installation outside files/ are protected."
    })
}

pub fn redact_internal(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in [
                "access_hash",
                "auth_key",
                "api_hash",
                "phone_code_hash",
                "file_reference",
                "secret_chat_key",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                redact_internal(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_internal(value);
            }
        }
        _ => {}
    }
}
