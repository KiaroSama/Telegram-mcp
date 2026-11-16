//! Telegram Desktop pixels and chat exports have separate native backends.

pub mod capture;
pub mod frames;
pub mod images;
pub mod lottie;
#[cfg(windows)]
mod win32;

use anyhow::Result;
use serde_json::Value;

pub const TOOL_NAMES: &[&str] = &[
    "list_telegram_windows",
    "get_telegram_screen",
    "get_telegram_region",
    "get_telegram_frames",
    "export_chat_history",
    "export_recent_actions",
    "export_status",
    "cancel_export",
];

pub async fn call(
    api: &dyn crate::backend::Telegram,
    name: &str,
    args: &Value,
) -> Option<Result<Value>> {
    if let Some(result) = crate::export::call(api, name, args).await {
        return Some(result);
    }
    match name {
        "list_telegram_windows"
        | "get_telegram_screen"
        | "get_telegram_region"
        | "get_telegram_frames" => Some(capture::request(name, args).await),
        _ => None,
    }
}
