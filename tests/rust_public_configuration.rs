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
        Sha256::digest(source_text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
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
        Sha256::digest(process_text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
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

fn selected_cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("TELEGRAM_") || name.starts_with("MCP_") {
            command.env_remove(key);
        }
    }
    command
        .env("TOKIO_WORKER_THREADS", "2")
        .env("XDG_STATE_HOME", root.join("unused-state-home"));
    command
}

#[tokio::test]
async fn explicit_subcommand_source_wins_over_environment_and_global_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ambient = root.join("ambient.env");
    let explicit = root.join("explicit-α.env");
    let wrong_state = root.join("ambient-state");
    let right_state = root.join("explicit-state");
    std::fs::create_dir(&right_state).unwrap();
    std::fs::create_dir(root.join("files")).unwrap();
    std::fs::write(right_state.join("work.session.json"), b"{}").unwrap();
    for (path, state) in [(&ambient, &wrong_state), (&explicit, &right_state)] {
        std::fs::write(
            path,
            format!(
                "TELEGRAM_SESSION_NAME_WORK=work\nTELEGRAM_STATE_DIR={}\n",
                serde_json::to_string(state.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
    }
    for global_flag in [false, true] {
        let mut command = selected_cli(root);
        command.env("TELEGRAM_ENV_FILE", &ambient);
        if global_flag {
            command.arg("--config").arg(&ambient);
        }
        command
            .args(["session-health", "--env-file"])
            .arg(&explicit)
            .args(["--key", "TELEGRAM_SESSION_NAME_WORK", "--json"]);
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["status"], "unknown",
            "The explicit source's existing native snapshot must be inspected"
        );
        assert!(
            !wrong_state.exists(),
            "An overridden environment source cannot create private state"
        );
    }
}

#[tokio::test]
async fn native_bind_address_uses_the_selected_file_and_process_precedence_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("selected.env");
    std::fs::write(
        &source,
        b"BIND_HOST=::1\nMCP_HOST=${BIND_HOST}\nMCP_PORT=18461\n",
    )
    .unwrap();
    let mut command = selected_cli(temp.path());
    command.args(["--config"]).arg(&source).arg("bind-address");
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[::1]:18461"
    );
    let mut command = selected_cli(temp.path());
    command
        .arg("--config")
        .arg(&source)
        .arg("bind-address")
        .env("MCP_PORT", "18462");
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[::1]:18462"
    );
    assert!(!temp.path().join("unused-state-home").exists());
}

#[tokio::test]
async fn account_removal_cli_archives_the_session_and_preserves_unrelated_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("selected.env");
    let state = root.join("private");
    std::fs::create_dir(&state).unwrap();
    let private_hash = "removal-api-hash-canary";
    let private_session = "removal-session-data-canary";
    let runtime_log = root.join("removal-runtime.log");
    std::fs::write(
        &source,
        format!(
            "TELEGRAM_SESSION_NAME_WORK=work\nTELEGRAM_API_HASH={private_hash}\nOTHER=preserved\n"
        ),
    )
    .unwrap();
    std::fs::write(
        state.join("work.session.json"),
        serde_json::to_vec(&serde_json::json!({"private_session":private_session})).unwrap(),
    )
    .unwrap();
    let mut command = selected_cli(root);
    command
        .args(["remove-account", "--account", "work", "--env-file"])
        .arg(&source)
        .args(["--key", "TELEGRAM_SESSION_NAME_WORK", "--state-dir"])
        .arg(&state)
        .env("TELEGRAM_MCP_LOG_FILE", &runtime_log)
        .env("RUST_LOG", "trace");
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["account", "archived", "moved", "removed"]
            .into_iter()
            .collect()
    );
    for bytes in [&output.stdout, &output.stderr] {
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(!text.contains(private_hash) && !text.contains(private_session));
    }
    assert!(
        !runtime_log.exists(),
        "The metadata-only removal CLI must not start service logging"
    );
    assert_eq!(report["account"], "work");
    assert_eq!(report["removed"], true);
    let archived = report["archived"].as_str().unwrap();
    assert!(state.join(format!("{archived}.session.json")).is_file());
    let stored: Value = serde_json::from_slice(
        &std::fs::read(state.join(format!("{archived}.session.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["private_session"], private_session);
    assert!(!state.join("work.session.json").exists());
    assert!(!state.join("account-rename-transaction.json").exists());
    let remaining = std::fs::read_to_string(source).unwrap();
    assert!(remaining.contains("OTHER=preserved"));
    assert!(!remaining.contains("TELEGRAM_SESSION_NAME_WORK"));
}
