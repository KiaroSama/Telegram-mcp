use super::*;
use std::path::Path;

fn writer(path: &Path, max: u64) -> Writer {
    let file = private_open(path).unwrap();
    let bytes = file.metadata().unwrap().len();
    Writer(Arc::new(Mutex::new(Destination {
        path: path.to_owned(),
        file: Some(file),
        bytes,
        max,
    })))
}

fn generations(path: &Path) -> Vec<PathBuf> {
    std::iter::once(path.to_owned())
        .chain((1..=3).map(|n| PathBuf::from(format!("{}.{n}", path.display()))))
        .filter(|path| path.exists())
        .collect()
}

#[test]
fn append_preserves_existing_records_and_flushes_new_records() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    std::fs::write(&path, b"older record\n").unwrap();
    let mut log = writer(&path, 1024);
    log.write_all(b"new record\n").unwrap();
    log.flush().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"older record\nnew record\n");
}

#[test]
fn rotation_keeps_three_generations_and_drops_only_oldest() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let mut log = writer(&path, 4);
    for record in [b"1111", b"2222", b"3333", b"4444", b"5555"] {
        log.write_all(record).unwrap();
    }
    log.flush().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"5555");
    for (generation, expected) in [(1, b"4444"), (2, b"3333"), (3, b"2222")] {
        assert_eq!(
            std::fs::read(format!("{}.{generation}", path.display())).unwrap(),
            expected
        );
    }
    assert!(!PathBuf::from(format!("{}.4", path.display())).exists());
}

#[test]
fn a_single_large_write_cannot_exceed_file_or_total_retention_bounds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let mut log = writer(&path, 128);
    log.write_all(&[b'x'; 128 * 10 + 17]).unwrap();
    log.flush().unwrap();
    let lengths: Vec<u64> = generations(&path)
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .collect();
    assert!(
        lengths.iter().all(|length| *length <= 128),
        "One log write bypassed the per-file bound: {lengths:?}"
    );
    assert!(lengths.iter().sum::<u64>() <= 128 * 4);
}

#[test]
fn rotation_does_not_split_utf8_code_points_between_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let mut log = writer(&path, 64);
    log.write_all("سلام 😀 می‌کند\n".repeat(20).as_bytes())
        .unwrap();
    log.flush().unwrap();
    for path in generations(&path) {
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() <= 64);
        assert!(std::str::from_utf8(&bytes).is_ok());
    }
}

#[test]
fn dropping_the_last_writer_does_not_discard_the_final_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    {
        let mut log = writer(&path, 1024);
        log.write_all(b"final runtime event\n").unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"final runtime event\n");
}

#[test]
fn writer_clones_serialize_complete_records_without_interleaving() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let log = writer(&path, 1020);
    let threads: Vec<_> = (0..4)
        .map(|worker| {
            let mut writer = log.clone();
            std::thread::spawn(move || {
                for record in 0..100 {
                    writer
                        .write_all(format!("{worker}:{record:03}\n").as_bytes())
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let record_pattern = regex::Regex::new(r"^[0-3]:[0-9]{3}$").unwrap();
    for path in generations(&path) {
        for line in std::fs::read_to_string(path).unwrap().lines() {
            assert!(record_pattern.is_match(line), "Interleaved record: {line}");
        }
    }
}

#[test]
fn newly_created_logs_are_owner_private() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let mut log = writer(&path, 32);
    log.write_all(b"private event\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(windows)]
    {
        assert!(std::fs::metadata(&path).unwrap().is_file());
    }
}

#[test]
fn reopened_existing_unix_log_permissions_are_tightened() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    std::fs::write(&path, b"existing\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let _log = writer(&path, 1024);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

fn subprocess(
    name: &str,
    log_path: Option<&str>,
    limit: Option<&str>,
) -> Option<(tempfile::TempDir, std::process::Output)> {
    if std::env::var("TELEGRAM_LOG_TEST_CHILD").as_deref() == Ok(name) {
        return None;
    }
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env("TELEGRAM_LOG_TEST_CHILD", name)
        .env("TELEGRAM_STATE_DIR", &state)
        .env_remove("TELEGRAM_MCP_LOG_FILE")
        .env_remove("TELEGRAM_MCP_LOG_MAX_BYTES");
    if let Some(path) = log_path {
        command.env("TELEGRAM_MCP_LOG_FILE", path);
    }
    if let Some(limit) = limit {
        command.env("TELEGRAM_MCP_LOG_MAX_BYTES", limit);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Logging subprocess exceeded its deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some((temp, output))
}

#[test]
fn runtime_file_never_copies_stdout_stderr_or_dependency_credentials() {
    let name = "logging::tests::runtime_file_never_copies_stdout_stderr_or_dependency_credentials";
    if let Some((temp, output)) = subprocess(name, None, None) {
        let log = std::fs::read_to_string(temp.path().join("state/mcp.log")).unwrap();
        assert!(log.contains("Runtime application sentinel"));
        for forbidden in [
            "stdout-protocol-sentinel",
            "raw-stderr-secret-sentinel",
            "dependency-secret-sentinel",
        ] {
            assert!(!log.contains(forbidden));
        }
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Runtime application sentinel"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("stdout-protocol-sentinel"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("raw-stderr-secret-sentinel"));
        return;
    }
    initialize().unwrap();
    tracing::info!(target:"telegram_mcp::logging_probe","Runtime application sentinel");
    tracing::error!(target:"grammers_mtsender::transport","dependency-secret-sentinel");
    println!("stdout-protocol-sentinel");
    eprintln!("raw-stderr-secret-sentinel");
}

#[test]
fn explicitly_disabled_file_logging_uses_only_stderr_and_creates_no_state() {
    let name =
        "logging::tests::explicitly_disabled_file_logging_uses_only_stderr_and_creates_no_state";
    if let Some((temp, output)) = subprocess(name, Some(""), None) {
        assert!(!temp.path().join("state").exists());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Disabled-file application sentinel")
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("Disabled-file application sentinel")
        );
        return;
    }
    initialize().unwrap();
    tracing::warn!(target:"telegram_mcp::logging_probe","Disabled-file application sentinel");
}

#[test]
fn invalid_log_size_configuration_is_refused_before_opening_a_log() {
    let name = "logging::tests::invalid_log_size_configuration_is_refused_before_opening_a_log";
    if let Some((temp, _)) = subprocess(name, None, Some("0")) {
        assert!(!temp.path().join("state/mcp.log").exists());
        return;
    }
    assert!(initialize().unwrap_err().to_string().contains("limit"));
}

#[test]
fn a_log_outside_private_state_is_refused_without_creating_it() {
    let name = "logging::tests::a_log_outside_private_state_is_refused_without_creating_it";
    if let Some((temp, _)) = subprocess(name, Some("outside-private-state.log"), None) {
        assert!(!temp.path().join("outside-private-state.log").exists());
        return;
    }
    assert!(
        initialize()
            .unwrap_err()
            .to_string()
            .contains("private state")
    );
}

#[test]
fn production_tracing_events_do_not_log_raw_values_or_format_errors() {
    fn scan(
        path: &Path,
        offenders: &mut Vec<String>,
        pattern: &regex::Regex,
        allowed: &regex::Regex,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, offenders, pattern, allowed);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && path.file_name().is_none_or(|name| name != "tests.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                for found in pattern.find_iter(&text) {
                    let mut depth = 1;
                    let mut quoted = false;
                    let mut escape = false;
                    let mut end = found.end();
                    for (index, ch) in text[found.end()..].char_indices() {
                        end = found.end() + index;
                        if quoted {
                            if escape {
                                escape = false;
                            } else if ch == '\\' {
                                escape = true;
                            } else if ch == '"' {
                                quoted = false;
                            }
                            continue;
                        }
                        if ch == '"' {
                            quoted = true;
                        } else if ch == '(' {
                            depth += 1;
                        } else if ch == ')' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                    let body = &text[found.end()..end];
                    if !allowed.is_match(body) {
                        offenders.push(format!(
                            "{}:{}",
                            path.display(),
                            text[..found.start()].lines().count()
                        ));
                    }
                }
            }
        }
    }
    let mut offenders = Vec::new();
    let pattern = regex::Regex::new(r"tracing::(?:error|warn|info|debug|trace)!\s*\(").unwrap();
    let allowed=regex::Regex::new(r#"(?s)^\s*(?:(?:operation\s*=\s*"[a-z_]+"|version\s*=\s*env!\("CARGO_PKG_VERSION"\)|action)\s*,\s*)?"[^"\\]*"\s*,?\s*$"#).unwrap();
    scan(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut offenders,
        &pattern,
        &allowed,
    );
    assert!(
        offenders.is_empty(),
        "Raw data can reach runtime tracing outside the bounded event contract: {}",
        offenders.join(", ")
    );
}

#[test]
fn logging_recovers_after_rotation_replacement_open_fails() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.log");
    let mut log = writer(&path, 4);
    log.write_all(b"old!").unwrap();
    // This is the exact durable intermediate state after rename succeeded but
    // opening the replacement failed. The filesystem fault has now cleared.
    {
        let mut destination = log.0.lock().unwrap();
        drop(destination.file.take());
        std::fs::rename(&path, path.with_extension("log.1")).unwrap();
    }
    log.write_all(b"next").unwrap();
    log.flush().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"next");
    assert_eq!(
        std::fs::read(path.with_extension("log.1")).unwrap(),
        b"old!"
    );
}
