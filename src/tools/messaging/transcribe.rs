//! Explicit, bounded voice/video-note transcription. Telegram is the default;
//! hosted engines are selected/configured deliberately and report data upload.
mod hosted;
mod settings;
mod whisper;
use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
pub use settings::Settings;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

type Flight = Arc<tokio::sync::Mutex<Option<Value>>>;
type Flights = HashMap<String, (Instant, Flight)>;
const NOTE: &str =
    "Machine transcript, not a verbatim quote; names, punctuation and words may be inaccurate.";

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "transcribe_voice" => Some(
            async {
                let snapshot = crate::account_manager::current_config()?;
                let settings = Settings::from_values(&snapshot.values, a)?;
                transcribe_with(api, &settings, a).await
            }
            .await,
        ),
        "transcribe_audio" => Some(
            async { transcribe_audio_with_state(crate::state::State::global()?.as_ref(), a).await }
                .await,
        ),
        "rate_voice_transcription" => Some(rate(api, a).await),
        _ => None,
    }
}
fn flight(key: String) -> Result<Flight> {
    static CACHE: OnceLock<Mutex<Flights>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Transcript cache is unavailable"))?;
    cache.retain(|_, (created, value)| {
        created.elapsed() < Duration::from_secs(3600) || Arc::strong_count(value) > 1
    });
    if let Some((_, flight)) = cache.get(&key) {
        return Ok(Arc::clone(flight));
    }
    if cache.len() >= 256 {
        let oldest = cache
            .iter()
            .filter(|(_, (_, value))| Arc::strong_count(value) == 1)
            .min_by_key(|(_, (created, _))| *created)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            cache.remove(&oldest);
        } else {
            anyhow::bail!("Transcript concurrency capacity reached; retry later")
        }
    }
    let flight = Arc::new(tokio::sync::Mutex::new(None));
    cache.insert(key, (Instant::now(), Arc::clone(&flight)));
    Ok(flight)
}

pub(crate) async fn transcribe_audio_with_state(
    state: &crate::state::State,
    a: &Value,
) -> Result<Value> {
    let snapshot = crate::account_manager::current_config()?;
    let settings = Settings::from_values(&snapshot.values, &json!({"engine":"whisper"}))?;
    transcribe_audio_with(state, &settings, a).await
}

/// Transcribe an allowed local file without accessing Telegram or a hosted provider.
async fn transcribe_audio_with(
    state: &crate::state::State,
    settings: &Settings,
    a: &Value,
) -> Result<Value> {
    let path = std::path::Path::new(args::text(a, "file_path")?);
    if settings.disabled {
        return Ok(json!({"transcribed":false,"reason":"transcription_disabled"}));
    }
    ensure!(
        settings.engine == "whisper",
        "Local audio requires native Whisper"
    );
    let path = state.resolve_file(path, crate::state::FileAccess::Read)?;
    settings.preflight()?;
    let deadline = tokio::time::Instant::now() + settings.timeout;
    let mut result = tokio::time::timeout_at(
        deadline,
        whisper::transcribe_file(state, settings, &path, deadline),
    )
    .await
    .context("Local transcription exceeded its time budget")??;
    result["source"] = json!("whisper");
    result["model"] = json!(settings.display_model());
    result["note"] = json!(NOTE);
    result["untrusted_content"] = json!(true);
    Ok(result)
}

/// Injectable settings make endpoint/privacy and concurrency behavior testable
/// without mutating process configuration or using live transcription accounts.
pub async fn transcribe_with(api: &dyn Telegram, settings: &Settings, a: &Value) -> Result<Value> {
    let mid = id(a, "message_id")?;
    if settings.disabled {
        return Ok(json!({"transcribed":false,"reason":"transcription_disabled"}));
    }
    let peer = args::peer(api, a, "chat_id").await?;
    let message = fetch(api, &peer, mid).await?;
    let document = message["media"]
        .get("document")
        .context("Message has no voice message or video note")?;
    let attributes = document["attributes"]
        .as_array()
        .context("Message has no voice/video-note attributes")?;
    let voice = attributes.iter().find(|attr| {
        attr["_"].as_str() == Some("documentAttributeAudio")
            && attr["voice"].as_bool().unwrap_or(false)
    });
    let video = attributes.iter().find(|attr| {
        attr["_"].as_str() == Some("documentAttributeVideo")
            && attr["round_message"].as_bool().unwrap_or(false)
    });
    ensure!(
        voice.is_some() || video.is_some(),
        "Message has no voice message or video note to transcribe"
    );
    if let Some(topic) = a.get("topic_id").filter(|v| !v.is_null()) {
        let wanted = args::integer(&json!({"topic":topic}), "topic", 0)?;
        let actual = if message["reply_to"]["forum_topic"]
            .as_bool()
            .unwrap_or(false)
        {
            message["reply_to"]
                .get("reply_to_top_id")
                .or_else(|| message["reply_to"].get("reply_to_msg_id"))
                .and_then(Value::as_i64)
                .or(Some(1))
        } else {
            Some(1)
        };
        ensure!(
            wanted > 0 && actual == Some(wanted),
            "Voice message does not belong to the selected forum topic"
        );
    }
    settings.preflight()?;
    let duration = voice
        .or(video)
        .and_then(|attr| attr.get("duration"))
        .cloned()
        .unwrap_or(Value::Null);
    let key=api.cache_key().map(|session|crate::lowercase_hex(&Sha256::digest(json!({"session":session,"peer":peer,"message_id":mid,"document_id":document["id"],"engine":settings.engine,"model":settings.cache_model(),"language":settings.language,"endpoint":settings.endpoint.as_ref().map(url::Url::as_str)}).to_string().as_bytes())));
    let deadline = tokio::time::Instant::now() + settings.timeout;
    let bucket = key.map(flight).transpose()?;
    let mut cached = if let Some(bucket) = &bucket {
        Some(
            tokio::time::timeout_at(deadline, bucket.lock())
                .await
                .context(
                    "Transcription exceeded its time budget while waiting for the same recording",
                )?,
        )
    } else {
        None
    };
    if let Some(value) = cached.as_ref().and_then(|value| value.as_ref()) {
        let mut value = value.clone();
        value["cached"] = json!(true);
        return Ok(value);
    }
    ensure!(
        tokio::time::Instant::now() < deadline,
        "Transcription exceeded its time budget while waiting for the same recording"
    );
    let engine = async {
        Ok::<Value, anyhow::Error>(match settings.engine.as_str() {
            "telegram" => native(api, &peer, mid).await?,
            "groq" | "openai" => {
                hosted::transcribe(api, settings, document, video.is_some()).await?
            }
            "whisper" => whisper::transcribe(api, settings, document, deadline).await?,
            _ => unreachable!(),
        })
    };
    let mut value=tokio::time::timeout_at(deadline,engine).await.context("Transcription exceeded its time budget; a hosted provider may still be processing the selected recording")??;
    value["source"] = json!(settings.engine);
    value["model"] = json!(settings.display_model());
    value["duration"] = duration;
    value["note"] = json!(NOTE);
    value["cached"] = json!(false);
    if value["transcribed"].as_bool() == Some(true)
        && let Some(cached) = &mut cached
    {
        **cached = Some(value.clone());
    }
    Ok(value)
}
async fn native(api: &dyn Telegram, peer: &Value, mid: i64) -> Result<Value> {
    for attempt in 0..=10 {
        let result = api
            .invoke(
                "messages.transcribeAudio",
                json!({"peer":peer,"msg_id":mid}),
            )
            .await?;
        if !result["pending"].as_bool().unwrap_or(false) {
            let text = result["text"]
                .as_str()
                .context("Telegram omitted transcript text")?;
            ensure!(
                !text.trim().is_empty() && text.len() <= 256 * 1024,
                "Telegram returned empty or oversized transcript text"
            );
            return Ok(
                json!({"transcribed":true,"text":text,"transcription_id":result["transcription_id"],"trial_remains_num":result["trial_remains_num"],"trial_remains_until_date":result["trial_remains_until_date"],"audio_leaves_machine":false,"processing":"telegram"}),
            );
        }
        if attempt == 10 {
            return Ok(
                json!({"transcribed":false,"reason":"pending","transcription_id":result["transcription_id"],"detail":"Telegram is still processing; retry shortly"}),
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    unreachable!()
}
async fn rate(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let mid = id(a, "message_id")?;
    let transcription = args::integer(a, "transcription_id", 0)?;
    ensure!(
        transcription != 0,
        "transcription_id must be the nonzero id returned by Telegram"
    );
    let good = args::required(a, "good")?
        .as_bool()
        .context("good must be a boolean")?;
    let peer = args::peer(api, a, "chat_id").await?;
    api.invoke(
        "messages.rateTranscribedAudio",
        json!({"peer":peer,"msg_id":mid,"transcription_id":transcription.to_string(),"good":good}),
    )
    .await
}
