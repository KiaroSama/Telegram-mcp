use crate::{args::text, backend::Telegram};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub const TOOL_NAMES: &[&str] = &[
    "start_secret_auto_save",
    "stop_secret_auto_save",
    "delete_secret_chat",
    "delete_secret_chat_both_sides",
    "delete_saved_secret_messages",
    "export_secret_chat",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some(
        async {
            let mut payload = payload(name, args)?;
            if name == "export_secret_chat" {
                let missing = missing_export_choices(args);
                if !missing.is_empty() {
                    return Ok(json!({"status":"choices_required","required":missing}));
                }
                if !args["peer_user"].is_null() {
                    let input = &args["peer_user"];
                    ensure!(
                        input.as_i64().is_some_and(|v| v > 0)
                            || input.as_str().is_some_and(|v| !v.trim().is_empty()),
                        "peer_user must be a user ID or username"
                    );
                    let peer = api.resolve_peer(input).await?;
                    ensure!(
                        peer["_"] == "inputPeerUser",
                        "peer_user must identify a user"
                    );
                    payload["peer_user"] = json!(
                        peer["user_id"]
                            .as_i64()
                            .or_else(|| peer["user_id"].as_str()?.parse().ok())
                            .filter(|v| *v > 0)
                            .context("Invalid export peer ID")?
                    );
                }
            }
            api.secret_call(name, payload).await
        }
        .await,
    )
}

/// Call before schema defaults are applied; an explicit empty date or folder is a choice.
pub fn missing_export_choices(args: &Value) -> Vec<&'static str> {
    [
        "format",
        "photos",
        "videos",
        "voice_messages",
        "video_messages",
        "stickers",
        "gifs",
        "files",
        "size_limit_mb",
        "date_from",
        "date_to",
        "destination",
    ]
    .into_iter()
    .filter(|field| args[*field].is_null())
    .collect()
}

fn payload(name: &str, args: &Value) -> Result<Value> {
    let mut payload = args
        .as_object()
        .cloned()
        .context("Tool arguments must be an object")?;
    payload.remove("ctx");
    match name {
        "start_secret_auto_save" => {
            ensure!(
                !text(args, "folder")?.trim().is_empty(),
                "folder must not be empty"
            );
        }
        "stop_secret_auto_save" => {}
        _ => {
            let id = super::secret_id(args, "secret_chat_id")?;
            payload.insert("secret_chat_id".into(), json!(id));
            if matches!(name, "delete_saved_secret_messages" | "export_secret_chat") {
                payload.insert("chat_id".into(), json!(id));
            }
        }
    }
    if name == "export_secret_chat" {
        ensure!(
            args["format"].is_null()
                || matches!(
                    args["format"].as_str(),
                    Some("html" | "json" | "html_and_json")
                ),
            "format must be html, json or html_and_json"
        );
        if !args["size_limit_mb"].is_null() {
            ensure!(
                args["size_limit_mb"]
                    .as_f64()
                    .is_some_and(|size| (1.0..=4000.0).contains(&size)),
                "size_limit_mb must be from 1 to 4000"
            );
        }
    }
    Ok(Value::Object(payload))
}
