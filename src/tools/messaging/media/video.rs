use super::{
    decoder, downloads,
    emoji_previews::{self, VideoProbe},
};
use crate::{
    args,
    backend::Telegram,
    state::State,
    tools::messaging::{
        helpers::{fetch, id},
        transcribe,
    },
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{Instant, timeout_at},
};

const MAX_PIXELS: u64 = 8192 * 4320;
const MIB: u64 = 1024 * 1024;
static VIDEO: Semaphore = Semaphore::const_new(1);

pub(super) async fn view(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let state = State::global()?;
    view_in(api, &state, a).await
}

async fn view_in(api: &dyn Telegram, state: &State, a: &Value) -> Result<Value> {
    let message_id = id(a, "message_id")?;
    let start = seconds(a, "start")?.unwrap_or(0.0);
    let end = seconds(a, "end")?;
    ensure!(end.is_none_or(|end| end > start), "end must be after start");
    let count = args::limit(a, "max_frames", 8, 10)?;
    let transcribe = args::boolean(a, "transcribe", true)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let _permit = timeout_at(deadline, VIDEO.acquire())
        .await
        .context("Video decoder is busy")??;
    let peer = args::peer(api, a, "chat_id").await?;
    let message = fetch(api, &peer, message_id).await?;
    validate_media(&message)?;
    if let Some(channel) = super::super::helpers::channel(&peer) {
        let details = api
            .invoke("channels.getChannels", json!({"id":[channel]}))
            .await?;
        let chat = details["chats"]
            .as_array()
            .and_then(|chats| {
                chats
                    .iter()
                    .find(|chat| chat["_"] == "channel" && chat["id"] == peer["channel_id"])
            })
            .context("Cannot verify the video's channel protection settings")?;
        ensure!(
            chat["noforwards"] != true,
            "This channel is protected from forwarding"
        );
    }
    if a.get("topic_id").is_some_and(|v| !v.is_null()) {
        crate::documents::check_message_topic(api, &peer, &message, id(a, "topic_id")?).await?;
    }
    let media = &message["media"];
    let cap = byte_limit()?;
    ensure!(
        downloads::declared_size(media).is_none_or(|size| size <= cap),
        "Video exceeds TELEGRAM_VIDEO_MAX_MB"
    );
    let root = state
        .file_roots()
        .first()
        .context("No allowed file roots")?;
    let work = tempfile::Builder::new()
        .prefix(".telegram-video-")
        .tempdir_in(root)?;
    crate::state::storage::restrict(work.path(), true)?;
    let input = work.path().join("source-video");
    timeout_at(
        deadline,
        api.download(downloads::download_location(media)?, &input, cap),
    )
    .await
    .context("Video download exceeded its time budget")??;
    let metadata = timeout_at(deadline, emoji_previews::probe(&input))
        .await
        .context("Video metadata probe exceeded its time budget")??;
    validate_dimensions(&metadata)?;
    let duration = metadata
        .duration
        .context("Telegram video has no reported duration")?;
    let end = end.unwrap_or(duration).min(duration);
    ensure!(start < end, "The requested interval is outside this video");
    let last = metadata.last_frame(end).max(start);
    let fallback = emoji_previews::frame_times(start, last, count);
    let (times, sampling) = match scene_times(&input, start, last, count, deadline).await {
        Ok(times) if !times.is_empty() => (times, "scene_changes"),
        _ => (fallback, "even_intervals"),
    };
    let images = emoji_previews::frames(&input, work.path(), &times, 512, "", deadline).await?;
    let transcript = if !transcribe {
        json!({"status":"not_requested"})
    } else if !metadata.audio {
        json!({"status":"no_audio"})
    } else {
        let audio = work.path().join("audio.wav");
        match timeout_at(deadline, transcript_clip(state, &input, &audio, start, end)).await {
            Ok(Ok(result)) => result,
            _ => json!({"status":"unavailable","reason":"Local transcription did not complete"}),
        }
    };
    let mut content = Vec::with_capacity(images.len() * 2);
    for (bytes, frame) in images {
        content.push(json!({"type":"text","text":format!("Frame at {} s", frame["timestamp"])}));
        content.push(json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(bytes)}));
    }
    Ok(
        json!({"content":content,"width":metadata.width,"height":metadata.height,"duration":duration,
        "start":start,"end":end,"frame_count":times.len(),"sampling":sampling,"transcript":transcript,"untrusted_content":true}),
    )
}

async fn scene_times(
    input: &Path,
    start: f64,
    last: f64,
    count: i64,
    deadline: Instant,
) -> Result<Vec<f64>> {
    use tokio::io::AsyncReadExt;
    let length = last - start;
    if count < 2 || length < 0.5 {
        return Ok(Vec::new());
    }
    // Sample at most 90 seconds. Long clips use uniform spacing across the full interval.
    if length > 90.0 {
        return Ok(Vec::new());
    }
    let (mut child, _tree) = crate::tools::visual::capture::containment::spawn(
        decoder()
            .args([
                "-nostdin",
                "-v",
                "info",
                "-max_alloc",
                "67108864",
                "-max_pixels",
                "35389440",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,ogg,avi",
                "-ss",
                &format!("{start:.6}"),
                "-i",
            ])
            .arg(input)
            .args([
                "-t",
                &format!("{length:.6}"),
                "-map",
                "0:V:0",
                "-an",
                "-vf",
                "fps=4,scale=320:-2,select=gt(scene\\,0.3),showinfo",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true),
    )?;
    let stderr = child
        .stderr
        .take()
        .context("Scene process has no output pipe")?;
    let scene_deadline = deadline.min(Instant::now() + Duration::from_secs(8));
    let bytes = timeout_at(scene_deadline, async {
        let mut bytes = Vec::new();
        stderr.take(128 * 1024 + 1).read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() <= 128 * 1024,
            "Scene output exceeded its byte limit"
        );
        ensure!(child.wait().await?.success(), "Scene detection failed");
        Ok::<_, anyhow::Error>(bytes)
    })
    .await??;
    Ok(select_scenes(
        &String::from_utf8_lossy(&bytes),
        start,
        last,
        count as usize,
    ))
}

fn select_scenes(log: &str, start: f64, last: f64, count: usize) -> Vec<f64> {
    let scenes = log
        .lines()
        .filter(|line| line.contains("Parsed_showinfo"))
        .filter_map(|line| {
            line.split_once("pts_time:")?
                .1
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .filter(|time| time.is_finite() && *time > 0.0 && start + time <= last)
        .map(|time| start + time)
        .collect::<Vec<_>>();
    if scenes.is_empty() {
        return scenes;
    }
    let mut times = vec![start];
    let wanted = (count - 1).min(scenes.len());
    for index in 0..wanted {
        times.push(scenes[index * scenes.len() / wanted]);
    }
    times
}

fn validate_media(message: &Value) -> Result<()> {
    let media = &message["media"];
    ensure!(
        message["noforwards"] != true && media["noforwards"] != true,
        "This video is protected from forwarding"
    );
    ensure!(
        !media["ttl_seconds"].as_i64().is_some_and(|ttl| ttl > 0),
        "This video is disappearing media"
    );
    ensure!(
        media["document"]["attributes"]
            .as_array()
            .is_some_and(|attributes| attributes
                .iter()
                .any(|attribute| attribute["_"] == "documentAttributeVideo")),
        "This message does not contain a video"
    );
    Ok(())
}

fn validate_dimensions(metadata: &VideoProbe) -> Result<()> {
    ensure!(
        metadata.width > 0
            && metadata.height > 0
            && metadata
                .width
                .checked_mul(metadata.height)
                .is_some_and(|pixels| pixels <= MAX_PIXELS),
        "Video exceeds the decoded pixel limit"
    );
    Ok(())
}

fn seconds(a: &Value, name: &str) -> Result<Option<f64>> {
    a.get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .with_context(|| format!("{name} must be finite and nonnegative"))
        })
        .transpose()
}

fn byte_limit() -> Result<u64> {
    let size = match std::env::var("TELEGRAM_VIDEO_MAX_MB") {
        Ok(value) => value
            .parse::<u64>()
            .context("TELEGRAM_VIDEO_MAX_MB must be an integer")?,
        Err(std::env::VarError::NotPresent) => 200,
        Err(error) => return Err(error.into()),
    };
    ensure!(
        (1..=512).contains(&size),
        "TELEGRAM_VIDEO_MAX_MB must be between 1 and 512"
    );
    Ok(size * MIB)
}

async fn transcript_clip(
    state: &State,
    input: &Path,
    audio: &Path,
    start: f64,
    end: f64,
) -> Result<Value> {
    let (mut child, _tree) = crate::tools::visual::capture::containment::spawn(
        decoder()
            .args([
                "-nostdin",
                "-v",
                "error",
                "-max_alloc",
                "67108864",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,ogg,avi",
                "-ss",
                &format!("{start:.6}"),
                "-i",
            ])
            .arg(input)
            .args([
                "-t",
                &format!("{:.6}", end - start),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-fs",
                "104857601",
            ])
            .arg(audio)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true),
    )?;
    let status = child.wait().await?;
    ensure!(status.success(), "Video audio extraction failed");
    let path = audio.to_str().context("Video work path is not UTF-8")?;
    let mut result =
        transcribe::transcribe_audio_with_state(state, &json!({"file_path":path})).await?;
    if let Some(segments) = result["segments"].as_array_mut() {
        for segment in segments {
            for key in ["start", "end"] {
                if let Some(offset) = segment[key].as_f64() {
                    segment[key] = json!((start + offset).min(end));
                }
            }
        }
    }
    result["status"] = if result["transcribed"] == false {
        json!("unavailable")
    } else if result["text"]
        .as_str()
        .is_some_and(|text| text.trim().is_empty())
    {
        json!("no_speech")
    } else {
        json!("complete")
    };
    result["timestamp_reference"] = json!("video_start");
    Ok(result)
}

#[cfg(test)]
#[path = "../../../../tests/unit/video_review.rs"]
mod tests;
