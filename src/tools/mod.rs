//! Domain adapters. An unknown tool is never treated as a successful operation.
pub mod advanced;
pub mod identity;
pub mod local;
pub mod messaging;
pub mod visual;
use crate::backend::Telegram;
use anyhow::{Result, bail};
use serde_json::Value;
pub const EXTRA_TOOL_NAMES: &[&str] = &["inspect_document", "prepare_upload"];
pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    match name {
        "inspect_document" => return crate::documents::inspect(api, args).await,
        "prepare_upload" => return crate::transfer::prepare(args),
        _ => {}
    }
    if let Some(result) = local::call(api, name, args).await {
        return result;
    }
    if let Some(result) = identity::call(api, name, args).await {
        return result;
    }
    if let Some(result) = messaging::call(api, name, args).await {
        return result;
    }
    if let Some(result) = advanced::call(api, name, args).await {
        return result;
    }
    if let Some(result) = visual::call(api, name, args).await {
        return result;
    }
    bail!("Tool has no native implementation: {name}")
}
