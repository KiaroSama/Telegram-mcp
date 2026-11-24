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
    Some(match payload(name, args) {
        Ok(payload) => api.secret_call(name, payload).await,
        Err(error) => Err(error),
    })
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
        if args["destination"].as_str() == Some("") {
            payload.remove("destination");
        }
    }
    Ok(Value::Object(payload))
}
