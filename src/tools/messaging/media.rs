pub mod downloads;
mod emoji_previews;
mod photos;
pub mod previews;
pub mod uploads;
mod video;
use crate::backend::Telegram;
use anyhow::Result;
pub use downloads::{download_bytes, download_location};
use serde_json::Value;

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    if let Some(result) = uploads::call(api, name, a).await {
        return Some(result);
    }
    if let Some(result) = downloads::call(api, name, a).await {
        return Some(result);
    }
    if let Some(result) = photos::call(api, name, a).await {
        return Some(result);
    }
    if name == "view_video" {
        return Some(video::view(api, a).await);
    }
    previews::call(api, name, a).await
}

/// Codec helpers receive no account, repository or API credentials.
pub fn child(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.starts_with("TELEGRAM_")
            || name == "GH_TOKEN"
            || name == "GITHUB_TOKEN"
            || name.ends_with("_API_KEY")
            || name.ends_with("_SECRET")
            || name.ends_with("_PASSWORD")
        {
            command.env_remove(key);
        }
    }
    command
}

/// Bound both decoder and filter workers before the caller adds any inputs.
pub fn decoder() -> tokio::process::Command {
    let mut command = child("ffmpeg");
    command.args([
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
    ]);
    command
}
