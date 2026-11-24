use super::{
    super::media::{self, downloads},
    Settings,
};
use crate::{backend::Telegram, state::State};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs::File, future::Future, path::Path, process::Stdio};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Semaphore, SemaphorePermit},
    time::Instant,
};

const MAX_AUDIO: u64 = 100 * 1024 * 1024;
const MAX_TRANSCRIPT: u64 = 256 * 1024;
static WHISPER: Semaphore = Semaphore::const_new(1);

enum Recording {
    Downloaded(Vec<u8>),
    Local(File),
}

pub(super) async fn transcribe(
    api: &dyn Telegram,
    settings: &Settings,
    document: &Value,
    deadline: Instant,
) -> Result<Value> {
    if let Some(size) = downloads::declared_size(document) {
        ensure!(
            size <= MAX_AUDIO,
            "Local transcription recording exceeds 100 MiB"
        );
    }
    let permit = WHISPER.acquire().await?;
    let audio =
        downloads::download_bytes(api, downloads::download_location(document)?, MAX_AUDIO).await?;
    let state = State::global()?;
    transcribe_recording(
        &state,
        settings,
        Recording::Downloaded(audio),
        permit,
        deadline,
    )
    .await
}

pub(super) async fn transcribe_file(
    state: &State,
    settings: &Settings,
    path: &Path,
    deadline: Instant,
) -> Result<Value> {
    let permit = WHISPER.acquire().await?;
    let file = crate::transfer::files::open_download(state, path)
        .context("Cannot open local audio file")?;
    ensure!(
        (1..=MAX_AUDIO).contains(&file.metadata()?.len()),
        "Local audio must be a nonempty file up to 100 MiB"
    );
    transcribe_recording(state, settings, Recording::Local(file), permit, deadline).await
}

async fn transcribe_recording(
    state: &State,
    settings: &Settings,
    recording: Recording,
    permit: SemaphorePermit<'static>,
    deadline: Instant,
) -> Result<Value> {
    let work = tempfile::Builder::new()
        .prefix("transcription-")
        .tempdir_in(state.root())?;
    let settings = settings.clone();
    run_worker(permit, async move {
        let input = work.path().join("recording");
        let segments = match recording {
            Recording::Downloaded(audio) => {
                tokio::fs::write(&input, audio).await?;
                false
            }
            Recording::Local(file) => {
                let mut reader = tokio::fs::File::from_std(file).take(MAX_AUDIO + 1);
                let mut output = tokio::fs::File::create(&input).await?;
                let copied = tokio::io::copy(&mut reader, &mut output).await?;
                ensure!(
                    (1..=MAX_AUDIO).contains(&copied),
                    "Local audio changed or exceeded 100 MiB"
                );
                output.flush().await?;
                true
            }
        };
        transcribe_recording_in(&settings, &input, work.path(), segments, deadline).await
    })
    .await
}

async fn run_worker<T: Send + 'static>(
    permit: SemaphorePermit<'static>,
    work: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    // The worker owns the permit until its native child has exited or been reaped.
    tokio::spawn(async move {
        let _permit = permit;
        work.await
    })
    .await
    .context("Local transcription worker stopped unexpectedly")?
}

async fn transcribe_recording_in(
    settings: &Settings,
    input: &Path,
    work: &Path,
    segments: bool,
    deadline: Instant,
) -> Result<Value> {
    let wav = work.join("recording.wav");
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
        .arg(input)
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
    process(&mut convert, deadline, &wav, MAX_AUDIO)
        .await
        .context("Local transcription audio conversion failed; install FFmpeg with the recording's codec")?;
    let output = work.join("transcript");
    let transcript_path = output.with_extension(if segments { "json" } else { "txt" });
    let mut command = command(settings, &wav, &output)?;
    if segments {
        command.arg("-oj");
    } else {
        command.args(["-otxt", "-nt"]);
    }
    tracing::info!(
        operation = "transcription_local",
        "Starting the configured local native Whisper executable"
    );
    process(&mut command, deadline, &transcript_path, MAX_TRANSCRIPT)
        .await
        .context(
            "Native Whisper transcription failed; verify the executable and local model format",
        )?;
    let transcript = tokio::fs::read_to_string(&transcript_path)
        .await
        .context("Native Whisper did not produce UTF-8 transcript text")?;
    let mut result = if segments {
        parse_segments(&transcript)?
    } else {
        let transcript = transcript.trim();
        ensure!(
            !transcript.is_empty(),
            "Native Whisper returned empty transcript text"
        );
        json!({"text":transcript})
    };
    result["transcribed"] = json!(true);
    result["audio_uploaded"] = json!(false);
    result["audio_leaves_machine"] = json!(false);
    result["processing"] = json!("local_native_whisper");
    Ok(result)
}

fn parse_segments(transcript: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(transcript).context("Native Whisper returned invalid JSON")?;
    let entries = value["transcription"]
        .as_array()
        .context("Native Whisper returned no segment list")?;
    let mut segments = Vec::new();
    let mut text = String::new();
    for entry in entries {
        let words = entry["text"]
            .as_str()
            .context("Native Whisper segment has no text")?;
        let words = words.trim();
        if words.is_empty() {
            continue;
        }
        let start = entry["offsets"]["from"]
            .as_u64()
            .context("Native Whisper segment has an invalid start time")?;
        let end = entry["offsets"]["to"]
            .as_u64()
            .context("Native Whisper segment has an invalid end time")?;
        ensure!(end >= start, "Native Whisper segment ends before it starts");
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(words);
        segments
            .push(json!({"start":start as f64 / 1000.0,"end":end as f64 / 1000.0,"text":words}));
    }
    Ok(json!({"text":text,"lang":value["result"]["language"].as_str(),"segments":segments}))
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
        .args(["-np", "-of"])
        .arg(output);
    command
        .arg("-l")
        .arg(settings.language.as_deref().unwrap_or("auto"));
    Ok(command)
}
async fn process(
    command: &mut tokio::process::Command,
    deadline: Instant,
    output: &Path,
    limit: u64,
) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "Native audio process exceeded its time budget"
    );
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Required native audio executable is unavailable")?;
    let error = loop {
        tokio::select! {
            status = child.wait() => {
                let status = match status {
                    Ok(status) => status,
                    Err(error) => break anyhow::Error::new(error).context("Could not wait for native audio process"),
                };
                ensure!(status.success(), "Native audio process exited unsuccessfully");
                let meta = std::fs::metadata(output).context("Native audio process did not produce its output file")?;
                ensure!(meta.is_file() && (1..=limit).contains(&meta.len()), "Native audio output is empty or exceeded its byte budget");
                return Ok(());
            }
            () = tokio::time::sleep_until(deadline) => break anyhow::anyhow!("Native audio process exceeded its time budget"),
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if std::fs::metadata(output).is_ok_and(|meta| meta.len() > limit) {
                    break anyhow::anyhow!("Native audio process exceeded its output byte budget");
                }
            }
        }
    };
    if child.kill().await.is_err() {
        child
            .wait()
            .await
            .context("Could not reap native audio process")?;
    }
    Err(error)
}

#[cfg(test)]
#[path = "../../../../tests/unit/messaging_review.rs"]
mod tests;

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn json_transcript_keeps_nonempty_segments_and_converts_milliseconds() {
        let fixture = json!({"result":{"language":"fa"},"transcription":[
            {"text":" سلام ","offsets":{"from":125,"to":1250}},
            {"text":"  ","offsets":{"from":1250,"to":1350}},
            {"text":"دنیا","offsets":{"from":1400,"to":2000}}
        ]});
        let result = parse_segments(&fixture.to_string()).unwrap();
        assert_eq!(result["text"], "سلام دنیا");
        assert_eq!(result["lang"], "fa");
        assert_eq!(
            result["segments"],
            json!([
                {"text":"سلام","start":0.125,"end":1.25},
                {"text":"دنیا","start":1.4,"end":2.0}
            ])
        );
        let silent = parse_segments(r#"{"transcription":[]}"#).unwrap();
        assert_eq!(silent["text"], "");
        assert_eq!(silent["segments"], json!([]));
    }

    #[test]
    fn json_transcript_rejects_bad_segment_times() {
        for offsets in [
            json!({"from":2,"to":1}),
            json!({"from":-1,"to":1}),
            json!({"from":"0","to":1}),
        ] {
            let fixture = json!({"transcription":[{"text":"word","offsets":offsets}]});
            assert!(parse_segments(&fixture.to_string()).is_err());
        }
    }

    #[tokio::test]
    async fn local_audio_rejects_protected_paths_and_oversized_files_before_launch() {
        let workspace = tempfile::tempdir().unwrap();
        let allowed = workspace.path().join("allowed");
        std::fs::create_dir(&allowed).unwrap();
        let state = State::at(workspace.path().join("private"), vec![allowed]).unwrap();
        let protected = state.root().join("recording.wav");
        std::fs::write(&protected, b"private fixture").unwrap();
        let oversized = state.file_roots()[0].join("oversized.wav");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_AUDIO + 1)
            .unwrap();
        let model = state.root().join("synthetic-model");
        std::fs::write(&model, b"synthetic fixture").unwrap();
        let values = std::collections::BTreeMap::from([
            ("TELEGRAM_TRANSCRIBE_ENGINE".into(), "whisper".into()),
            (
                "TELEGRAM_TRANSCRIBE_WHISPER_CLI".into(),
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "TELEGRAM_TRANSCRIBE_WHISPER_MODEL_PATH".into(),
                model.to_string_lossy().into_owned(),
            ),
        ]);
        let settings = Settings::from_values(&values, &json!({})).unwrap();
        for (path, reason) in [(protected, "protected"), (oversized, "100 MiB")] {
            let error =
                super::super::transcribe_audio_with(&state, &settings, &json!({"file_path":path}))
                    .await
                    .unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
        }
    }

    #[tokio::test]
    async fn cancelling_a_waiter_keeps_the_worker_permit_until_inference_finishes() {
        static SEMAPHORE: Semaphore = Semaphore::const_new(1);
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, finish) = tokio::sync::oneshot::channel();
        let permit = SEMAPHORE.try_acquire().unwrap();
        let waiter = tokio::spawn(run_worker(permit, async move {
            ready.send(()).unwrap();
            finish.await?;
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(3), started)
            .await
            .unwrap()
            .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(SEMAPHORE.try_acquire().is_err());
        release.send(()).unwrap();
        let next =
            tokio::time::timeout(std::time::Duration::from_secs(3), SEMAPHORE.acquire()).await;
        assert!(next.unwrap().is_ok());
    }
}
