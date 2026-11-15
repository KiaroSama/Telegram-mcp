use super::{super::media::downloads, Settings};
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const MAX_AUDIO: u64 = 25 * 1024 * 1024;
pub(super) async fn transcribe(
    api: &dyn Telegram,
    settings: &Settings,
    document: &Value,
    video: bool,
) -> Result<Value> {
    if let Some(size) = downloads::declared_size(document) {
        ensure!(
            size <= MAX_AUDIO,
            "Hosted transcription accepts at most 25MiB; select Telegram or use a shorter recording"
        );
    }
    let audio =
        downloads::download_bytes(api, downloads::download_location(document)?, MAX_AUDIO).await?;
    ensure!(
        !audio.is_empty() && audio.len() as u64 <= MAX_AUDIO,
        "Downloaded recording is empty or exceeds the 25MiB transcription limit"
    );
    let endpoint = settings
        .endpoint
        .as_ref()
        .context("Transcription endpoint is not configured")?;
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(if video { "video_note.mp4" } else { "voice.ogg" })
        .mime_str(if video { "video/mp4" } else { "audio/ogg" })?;
    let mut form = reqwest::multipart::Form::new()
        .text("model", settings.model.clone())
        .text("response_format", "json")
        .part("file", part);
    if let Some(language) = &settings.language {
        form = form.text("language", language.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(settings.timeout)
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Cannot initialize the transcription HTTP client")?;
    let mut request = client.post(endpoint.clone()).multipart(form);
    if let Some(key) = &settings.key {
        request = request.bearer_auth(key);
    }
    tracing::info!(
        operation = "transcription_upload",
        "Sending explicitly selected recording to its configured transcription endpoint"
    );
    let mut response = request.send().await.map_err(|_| {
        anyhow::anyhow!(
            "The configured transcription endpoint could not be reached within its time budget"
        )
    })?;
    ensure!(
        response.status().is_success(),
        "Transcription endpoint returned HTTP{}",
        response.status().as_u16()
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= 1_048_576),
        "Transcription response exceeds 1 MiB"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Transcription response could not be read"))?
    {
        ensure!(
            body.len() + chunk.len() <= 1_048_576,
            "Transcription response exceeds 1 MiB"
        );
        body.extend(chunk);
    }
    let payload: Value = serde_json::from_slice(&body)
        .context("Transcription endpoint returned invalid UTF-8 JSON")?;
    let text = payload["text"]
        .as_str()
        .context("Transcription endpoint omitted text")?
        .trim();
    ensure!(
        !text.is_empty() && text.len() <= 256 * 1024,
        "Transcription endpoint returned empty or oversized text"
    );
    let loopback = endpoint.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    Ok(
        json!({"transcribed":true,"text":text,"language":payload["language"],"audio_uploaded":true,"audio_leaves_machine":!loopback,"processing":"configured_endpoint"}),
    )
}
