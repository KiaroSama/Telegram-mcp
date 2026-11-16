//! Read Telegram documents without leaving a downloaded copy on disk.
mod worker;
use crate::{
    args,
    backend::Telegram,
    tools::messaging::{helpers, media},
    tools::visual::images,
};
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
pub use worker::worker_entry;
pub const MAX_DOCUMENT_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
static IMAGE_WORKERS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

async fn image_work<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = tokio::time::timeout(
        Duration::from_secs(5),
        Arc::clone(IMAGE_WORKERS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2))))
            .acquire_owned(),
    )
    .await
    .context("Image inspection workers are busy")?
    .context("Image inspection workers are unavailable")?;
    tokio::task::spawn_blocking(move || {
        // Cancellation of the async caller must not release capacity while
        // the bounded blocking decode/resize is still running.
        let _permit = permit;
        work()
    })
    .await
    .context("Image inspection worker failed")?
}

/// A trusted host dependency, separate from every MCP argument and environment
/// variable. Embedders may provide the native executable explicitly; the server
/// uses its own executable. The subprocess receives only document bytes.
pub struct PdfWorker {
    executable: PathBuf,
}
impl PdfWorker {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
    pub fn current() -> Result<Self> {
        Ok(Self::new(std::env::current_exe()?))
    }
    pub async fn extract(&self, bytes: &[u8]) -> Result<Value> {
        worker::extract(&self.executable, bytes).await
    }
}

pub async fn inspect(api: &dyn Telegram, args: &Value) -> Result<Value> {
    inspect_with_worker(api, args, &PdfWorker::current()?).await
}

pub async fn inspect_with_worker(
    api: &dyn Telegram,
    args: &Value,
    worker: &PdfWorker,
) -> Result<Value> {
    let topic = args
        .get("topic_id")
        .filter(|v| !v.is_null())
        .map(|value| {
            let id = value.as_i64().context("topic_id must be an integer")?;
            ensure!(
                (1..=i32::MAX as i64).contains(&id),
                "topic_id must be a positive 32-bit message ID"
            );
            Ok::<_, anyhow::Error>(id)
        })
        .transpose()?;
    let peer = args::peer(api, args, "chat_id").await?;
    let id = helpers::id(args, "message_id")?;
    let message = helpers::fetch(api, &peer, id).await?;
    if let Some(expected) = topic {
        check_message_topic(api, &peer, &message, expected).await?;
    }
    let media = message
        .get("media")
        .filter(|v| !v.is_null())
        .context("This message has no attached document or image")?;
    let raw = media
        .get("document")
        .or_else(|| media.get("photo"))
        .unwrap_or(media);
    let filename = raw["attributes"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["file_name"].as_str()))
        .unwrap_or("document");
    let mime = raw["mime_type"].as_str().unwrap_or_default();
    let suffix = filename
        .rsplit_once('.')
        .map(|(_, suffix)| suffix.to_ascii_lowercase())
        .unwrap_or_default();
    let pdf = mime == "application/pdf" || suffix == "pdf";
    let image = media.get("photo").is_some()
        || mime.starts_with("image/")
        || matches!(
            suffix.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        );
    let text = mime.starts_with("text/")
        || matches!(
            suffix.as_str(),
            "txt" | "csv" | "tsv" | "json" | "md" | "log" | "xml" | "yaml" | "yml" | "toml"
        );
    ensure!(
        pdf || image || text,
        "This attachment is not a supported image, PDF or text document"
    );
    ensure!(
        media::downloads::declared_size(media).is_none_or(|size| size <= MAX_DOCUMENT_BYTES),
        "Document exceeds the 25 MiB inspection budget"
    );
    let bytes =
        media::downloads::download_bytes(api, media::download_location(media)?, MAX_DOCUMENT_BYTES)
            .await?;
    ensure!(!bytes.is_empty(), "The document download was empty");
    let mut metadata = json!({"message_id":id,"filename":filename,"bytes":bytes.len(),"untrusted":true,"stored":false});
    if !text && image::guess_format(&bytes).is_ok() {
        let (data, details) = image_work(move || {
            let image = images::decode_image(&bytes)?;
            let (data, details) = images::encode_image(&image, "png", 1568, 90, false)?;
            Ok((
                base64::engine::general_purpose::STANDARD.encode(data),
                details,
            ))
        })
        .await?;
        metadata["kind"] = json!("image");
        metadata["image"] = details;
        return Ok(
            json!({"content":[{"type":"image","mimeType":"image/png","data":data}],"metadata":metadata}),
        );
    }
    let content = if pdf {
        ensure!(
            bytes.starts_with(b"%PDF-"),
            "Attachment is not a PDF despite its filename or MIME type"
        );
        worker.extract(&bytes).await?
    } else {
        ensure!(
            !image,
            "Image bytes could not be decoded as a supported image format"
        );
        let text = decode_text(&bytes)?;
        let (text, truncated) = bounded_text(text);
        json!({"text":text,"truncated":truncated})
    };
    metadata["kind"] = json!(if pdf { "pdf" } else { "text" });
    Ok(
        json!({"text":content["text"],"truncated":content["truncated"],"reason":content["reason"],"metadata":metadata}),
    )
}

pub(crate) async fn check_message_topic(
    api: &dyn Telegram,
    peer: &Value,
    message: &Value,
    expected: i64,
) -> Result<()> {
    let mut actual = message["reply_to"]["reply_to_top_id"].as_i64().or_else(|| {
        (message["reply_to"]["forum_topic"] == true)
            .then(|| message["reply_to"]["reply_to_msg_id"].as_i64().unwrap_or(1))
    });
    if actual.is_none()
        && message["_"] == "messageService"
        && message["action"]["_"] == "messageActionTopicCreate"
        && message["id"].as_i64() == Some(expected)
        && helpers::channel(peer).is_some()
        && helpers::same_peer(
            &message["peer_id"],
            &json!({"_":"peerChannel","channel_id":peer["channel_id"]}),
        )
    {
        actual = Some(expected);
    }
    if actual.is_none()
        && expected == 1
        && let Some(channel) = helpers::channel(peer)
    {
        let detail = api
            .invoke("channels.getFullChannel", json!({"channel":channel}))
            .await?;
        if detail["chats"].as_array().is_some_and(|chats| {
            chats.iter().any(|chat| {
                chat["_"] == "channel"
                    && chat["id"].to_string().trim_matches('"')
                        == peer["channel_id"].to_string().trim_matches('"')
                    && chat["forum"] == true
            })
        }) {
            // General (id 1) uses ordinary messages and ordinary replies;
            // they intentionally have no forum top-message metadata.
            actual = Some(1);
        }
    }
    ensure!(
        actual == Some(expected),
        "The message is outside the requested forum topic"
    );
    Ok(())
}
pub fn decode_text(bytes: &[u8]) -> Result<String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        ensure!(
            bytes.len().is_multiple_of(2),
            "UTF-16 document ends in a partial code unit"
        );
        let little = bytes[..2] == [0xff, 0xfe];
        let units = bytes[2..].as_chunks::<2>().0.iter().map(|pair| {
            if little {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        });
        return char::decode_utf16(units)
            .map(|value| value.context("UTF-16 document contains an unpaired surrogate"))
            .collect();
    }
    ensure!(
        !bytes.contains(&0),
        "Document appears to be binary data, not UTF-8 text"
    );
    String::from_utf8(bytes.to_vec())
        .context("Document is not UTF-8 text; convert its encoding explicitly")
}
pub(crate) fn bounded_text(mut text: String) -> (String, bool) {
    if text.len() <= MAX_TEXT_BYTES {
        return (text, false);
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}
