use super::{
    super::media::{self, downloads},
    Settings,
};
use crate::{backend::Telegram, state::State};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio};

pub(super) async fn transcribe(
    api: &dyn Telegram,
    settings: &Settings,
    document: &Value,
) -> Result<Value> {
    const MAX_AUDIO: u64 = 100 * 1024 * 1024;
    if let Some(size) = downloads::declared_size(document) {
        ensure!(
            size <= MAX_AUDIO,
            "Local transcription recording exceeds 100MiB"
        );
    }
    let audio =
        downloads::download_bytes(api, downloads::download_location(document)?, MAX_AUDIO).await?;
    let state = State::global()?;
    let work = tempfile::Builder::new()
        .prefix("transcription-")
        .tempdir_in(state.root())?;
    let input = work.path().join("recording");
    std::fs::write(&input, audio)?;
    let wav = work.path().join("recording.wav");
    let mut convert = media::decoder();
    convert
        .args([
            "-nostdin",
            "-v",
            "error",
            "-max_alloc",
            "67108864",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "ogg,mov,matroska,webm,mp3,aac,wav,flac",
            "-i",
        ])
        .arg(&input)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-sample_fmt",
            "s16",
            "-fs",
            "104857601",
        ])
        .arg(&wav);
    process(&mut convert,settings.timeout,&wav,MAX_AUDIO).await.context("Local transcription audio conversion failed; install FFmpeg with the recording's codec")?;
    let output = work.path().join("transcript");
    let text = work.path().join("transcript.txt");
    let mut command = command(settings, &wav, &output)?;
    tracing::info!(
        operation = "transcription_local",
        "Starting the configured local native Whisper executable"
    );
    process(&mut command, settings.timeout, &text, 256 * 1024)
        .await
        .context(
            "Native Whisper transcription failed; verify the executable and local model format",
        )?;
    let transcript = std::fs::read_to_string(&text)
        .context("Native Whisper did not produce UTF-8 transcript text")?;
    let transcript = transcript.trim();
    ensure!(
        !transcript.is_empty() && transcript.len() <= 256 * 1024,
        "Native Whisper returned empty or oversized transcript text"
    );
    Ok(
        json!({"transcribed":true,"text":transcript,"audio_uploaded":false,"audio_leaves_machine":false,"processing":"local_native_whisper"}),
    )
}
fn command(settings: &Settings, wav: &Path, output: &Path) -> Result<tokio::process::Command> {
    let mut command = media::child(
        settings
            .whisper_cli
            .as_ref()
            .context("Native whisper-cli is not configured")?
            .to_str()
            .context("Native whisper-cli path must be UTF-8")?,
    );
    command
        .arg("-m")
        .arg(
            settings
                .whisper_model
                .as_ref()
                .context("Local Whisper model is not configured")?,
        )
        .arg("-f")
        .arg(wav)
        .args(["-otxt", "-nt", "-np", "-of"])
        .arg(output);
    command
        .arg("-l")
        .arg(settings.language.as_deref().unwrap_or("auto"));
    Ok(command)
}
async fn process(
    command: &mut tokio::process::Command,
    timeout: std::time::Duration,
    output: &Path,
    limit: u64,
) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Required native audio executable is unavailable")?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        tokio::select! {
            status=child.wait()=>{ensure!(status?.success(),"Native audio process exited unsuccessfully");let meta=std::fs::metadata(output).context("Native audio process did not produce its output file")?;ensure!(meta.is_file()&&meta.len()>0&&meta.len()<=limit,"Native audio output is empty or exceeded its byte budget");return Ok(())},
            _=tokio::time::sleep_until(deadline)=>{let _=child.kill().await;anyhow::bail!("Native audio process exceeded its time budget")},
            _=tokio::time::sleep(std::time::Duration::from_millis(100))=>{if std::fs::metadata(output).is_ok_and(|meta|meta.len()>limit){let _=child.kill().await;anyhow::bail!("Native audio process exceeded its output byte budget")}},
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/messaging_review.rs"]
mod tests;
