//! Individual preview frames are required by Emoji Studio; a contact sheet
//! cannot be turned back into an honest animation or visual fingerprint.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::AsyncReadExt;

pub async fn individual(
    data: &[u8],
    count: i64,
    dimension: i64,
    mime: &str,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let suffix = if data.starts_with(b"\x1f\x8b") {
        Some("tgs")
    } else if data.starts_with(b"GIF8") {
        Some("gif")
    } else if data.starts_with(b"RIFF") && data.windows(4).any(|part| part == b"ANIM") {
        Some("webp")
    } else if data.starts_with(b"\x89PNG") && data.windows(4).any(|part| part == b"acTL") {
        Some("png")
    } else {
        None
    };
    let frames = if let Some(suffix) = suffix {
        let state = crate::state::State::global()?;
        let root = state
            .file_roots()
            .first()
            .context("No allowed file roots")?;
        let temp = tempfile::Builder::new()
            .prefix(".telegram-emoji-")
            .tempdir_in(root)?;
        let path = temp.path().join(format!("animation.{suffix}"));
        std::fs::write(&path, data)?;
        match crate::tools::visual::frames::extract_frames(&path, count.clamp(1, 10), dimension)
            .await
        {
            Ok(frames) => frames,
            Err(error) if error.to_string().contains("not animated") => {
                vec![still(data, dimension)?]
            }
            Err(error) => return Err(error),
        }
    } else if let Ok(image) = crate::tools::visual::images::decode_image(data) {
        vec![encode(&image, dimension)?]
    } else {
        video(data, count, dimension, mime).await?
    };
    let mut content = Vec::new();
    let mut previews = Vec::new();
    let mut total = 0;
    for (bytes, mut meta) in frames {
        total += bytes.len();
        ensure!(
            total <= 32 * 1024 * 1024,
            "Emoji frames exceed the aggregate image byte bound"
        );
        meta["blank"] = meta["transparent"].as_bool().unwrap_or(false).into();
        content.push(json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(bytes)}));
        previews.push(meta);
    }
    Ok((content, previews))
}

fn still(data: &[u8], dimension: i64) -> Result<(Vec<u8>, Value)> {
    encode(
        &crate::tools::visual::images::decode_image(data)?,
        dimension,
    )
}
fn encode(image: &image::DynamicImage, dimension: i64) -> Result<(Vec<u8>, Value)> {
    let (bytes, mut meta) =
        crate::tools::visual::images::encode_image(image, "png", dimension, 90, false)?;
    meta["transparent"] = image.to_rgba8().pixels().all(|pixel| pixel[3] == 0).into();
    Ok((bytes, meta))
}

async fn probe(path: &std::path::Path) -> Result<(Option<f64>, Option<f64>)> {
    let mut child = super::child("ffprobe")
        .args([
            "-v",
            "error",
            "-max_alloc",
            "67108864",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,ogg,avi",
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration:stream=duration,avg_frame_rate",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("FFprobe is required for video emoji previews")?;
    let stdout = child
        .stdout
        .take()
        .context("Video probe has no output pipe")?;
    let operation = async {
        let mut bytes = Vec::new();
        stdout.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            "Video probe output exceeds its byte bound"
        );
        ensure!(child.wait().await?.success(), "Video metadata probe failed");
        let value: Value = serde_json::from_slice(&bytes)?;
        let stream = value["streams"]
            .as_array()
            .and_then(|items| items.first())
            .unwrap_or(&Value::Null);
        let duration = value["format"]["duration"]
            .as_str()
            .or_else(|| stream["duration"].as_str())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0);
        let rate = stream["avg_frame_rate"]
            .as_str()
            .and_then(|value| value.split_once('/'))
            .and_then(|(a, b)| {
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                (a > 0.0 && b > 0.0).then_some(a / b)
            });
        Ok((duration, rate))
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), operation).await {
        Ok(result) => {
            if result.is_err() {
                let _ = child.kill().await;
            }
            result
        }
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("Video probe exceeded its time bound")
        }
    }
}

async fn video(
    data: &[u8],
    count: i64,
    dimension: i64,
    mime: &str,
) -> Result<Vec<(Vec<u8>, Value)>> {
    let state = crate::state::State::global()?;
    let root = state
        .file_roots()
        .first()
        .context("No allowed file roots")?;
    let temp = tempfile::Builder::new()
        .prefix(".telegram-emoji-video-")
        .tempdir_in(root)?;
    let input = temp.path().join("source-media");
    std::fs::write(&input, data)?;
    let (duration, rate) = probe(&input).await?;
    let count = count.clamp(1, 10);
    let end = duration
        .map(|duration| {
            (duration
                - rate
                    .filter(|rate| rate.is_finite() && *rate > 0.0)
                    .map(|rate| 1.0 / rate)
                    .unwrap_or(0.05))
            .max(0.0)
        })
        .unwrap_or(0.0);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut frames = Vec::new();
    for index in 0..count {
        let at = if count == 1 {
            0.0
        } else {
            end * index as f64 / (count - 1) as f64
        };
        let output = temp.path().join(format!("frame-{index}.png"));
        let mut command = super::decoder();
        command.args([
            "-nostdin",
            "-v",
            "error",
            "-max_alloc",
            "67108864",
            "-max_pixels",
            "50000000",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,ogg,avi",
        ]);
        if mime == "video/webm" {
            command.args(["-c:v", "libvpx-vp9"]);
        }
        command
            .args(["-ss", &format!("{at:.6}"), "-i"])
            .arg(&input)
            .args([
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-vf",
                &format!(
                    "scale={}:{}:force_original_aspect_ratio=decrease",
                    dimension.clamp(64, 1568),
                    dimension.clamp(64, 1568)
                ),
                "-pix_fmt",
                "rgba",
            ])
            .arg(&output);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("FFmpeg is required for video emoji previews")?;
        match tokio::time::timeout_at(deadline, child.wait()).await {
            Ok(status) => ensure!(
                status?.success(),
                "Video emoji decoding failed at {at:.3} seconds"
            ),
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("Video emoji decoding exceeded its request budget");
            }
        }
        ensure!(
            output.is_file() && output.metadata()?.len() <= 16 * 1024 * 1024,
            "Video decoder did not return a bounded frame at {at:.3} seconds"
        );
        let (bytes, mut meta) = still(&std::fs::read(output)?, dimension)?;
        meta["timestamp"] = json!(at);
        meta["source"] = "ffmpeg".into();
        frames.push((bytes, meta));
    }
    Ok(frames)
}
