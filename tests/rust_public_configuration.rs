use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::process::Command;

async fn cli(root: &Path, source: &str, process_source: Option<&str>) -> Output {
    let files = root.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(root.join(".env"), source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
    command
        .current_dir(root)
        .args(["accounts", "--json"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("TELEGRAM_") || name.starts_with("MCP_") {
            command.env_remove(key);
        }
    }
    command
        .env("TELEGRAM_STATE_DIR", root.join("state"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&[files.to_str().unwrap()]).unwrap(),
        )
        .env("TOKIO_WORKER_THREADS", "2")
        .env("RUST_LOG", "trace");
    if let Some(value) = process_source {
        command.env("TELEGRAM_SESSION_STRING_WORK", value);
    }
    tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("Public inventory exceeded its offline deadline")
        .unwrap()
}
fn assert_no_canaries(root: &Path, output: &Output, canaries: &[&str]) {
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for canary in canaries {
            assert!(
                !text.contains(canary),
                "A private source value reached public output"
            );
        }
    }
    let state = root.join("state");
    if !state.exists() {
        return;
    }
    for entry in std::fs::read_dir(state).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".session.json") && !name.starts_with("auth-"),
            "Public inventory created a Telegram authorization"
        );
        if name.contains(".log") {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            for canary in canaries {
                assert!(
                    !text.contains(canary),
                    "A private source value reached a runtime log"
                );
            }
        }
    }
}
#[tokio::test]
async fn public_inventory_cli_omits_raw_session_and_api_hash_in_stdout_stderr_and_logs() {
    let directory = tempfile::tempdir().unwrap();
    let source_text = "session-output-canary-1z3x5v7b9n";
    let api_hash = "api-hash-output-canary-2q4w6e8r0t";
    let output = cli(
        directory.path(),
        &format!("TELEGRAM_SESSION_STRING_WORK={source_text}\nTELEGRAM_API_HASH={api_hash}\n"),
        None,
    )
    .await;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_no_canaries(directory.path(), &output, &[source_text, api_hash]);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["accounts"][0]["config_digest"],
        format!("{:x}", Sha256::digest(source_text.as_bytes()))
    );
    assert_eq!(
        report["accounts"][0]["config_key"],
        "TELEGRAM_SESSION_STRING_WORK"
    );
}
#[tokio::test]
async fn process_source_precedence_retains_exact_revision_without_emitting_either_source() {
    let directory = tempfile::tempdir().unwrap();
    let file_text = "file-source-canary-must-stay-private";
    let process_text = "process-source-canary-must-stay-private";
    let output = cli(
        directory.path(),
        &format!("TELEGRAM_SESSION_STRING_WORK={file_text}\n"),
        Some(process_text),
    )
    .await;
    assert!(output.status.success());
    assert_no_canaries(directory.path(), &output, &[file_text, process_text]);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["accounts"][0]["config_digest"],
        format!("{:x}", Sha256::digest(process_text.as_bytes()))
    );
}
#[tokio::test]
async fn invalid_public_inventory_source_reports_no_private_values_on_error() {
    let directory = tempfile::tempdir().unwrap();
    let source_text = "invalid-label-source-canary-no-echo";
    let api_hash = "invalid-label-api-hash-canary-no-echo";
    let output = cli(
        directory.path(),
        &format!("TELEGRAM_SESSION_STRING_={source_text}\nTELEGRAM_API_HASH={api_hash}\n"),
        None,
    )
    .await;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_no_canaries(directory.path(), &output, &[source_text, api_hash]);
}
