#[test]
fn legacy_bare_dollar_names_remain_literal_while_braced_names_expand() {
    let values = parse_config(
        "ARCHIVE=archive\nBARE=/tmp/$ARCHIVE\nQUOTED=\"$ARCHIVE/${ARCHIVE}\"\n",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(values["BARE"], "/tmp/$ARCHIVE");
    assert_eq!(values["QUOTED"], "$ARCHIVE/archive");
}

#[test]
fn shared_bind_address_resolves_selected_file_interpolation_and_process_precedence() {
    if let Some(path) = std::env::var_os("TG_TEST_BIND_CONFIG") {
        telegram_mcp::account_manager::select_source(std::path::Path::new(&path)).unwrap();
        telegram_mcp::account_manager::capture_process_config();
        assert_eq!(
            telegram_mcp::account_manager::bind_address()
                .unwrap()
                .to_string(),
            std::env::var("TG_TEST_BIND_EXPECTED").unwrap()
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = dir.path().join("selected.env");
    std::fs::write(
        &env,
        "BIND_HOST=127.0.0.1\nMCP_HOST=${BIND_HOST}\nMCP_PORT=45678\n",
    )
    .unwrap();
    for override_process in [false, true] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "shared_bind_address_resolves_selected_file_interpolation_and_process_precedence",
                "--nocapture",
            ])
            .env("TG_TEST_BIND_CONFIG", &env)
            .env_remove("MCP_HOST")
            .env_remove("MCP_PORT");
        if override_process {
            child
                .env("MCP_HOST", "::1")
                .env("MCP_PORT", "9876")
                .env("TG_TEST_BIND_EXPECTED", "[::1]:9876");
        } else {
            child.env("TG_TEST_BIND_EXPECTED", "127.0.0.1:45678");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn dotenv_legacy_windows_paths_and_documented_quote_escapes_are_lossless() {
    let values = parse_config(
        r#"TELEGRAM_STATE_DIR=C:\Fixtures\Demo\state
OTHER="C:\Fixtures\Demo\state"
SINGLE='C:\Fixtures\Demo\state'
APOSTROPHE=C:\Fixtures\O'Neil\state
ESCAPED='it\'s \\ safe'
CONTROLS="\a\b\f\n\r\t\v\'\"\\\z"
"#,
        &BTreeMap::new(),
    )
    .unwrap();
    for key in ["TELEGRAM_STATE_DIR", "OTHER", "SINGLE"] {
        assert_eq!(values[key], r"C:\Fixtures\Demo\state");
    }
    assert_eq!(values["APOSTROPHE"], r"C:\Fixtures\O'Neil\state");
    assert_eq!(values["ESCAPED"], "it's \\ safe");
    assert_eq!(values["CONTROLS"], "\u{7}\u{8}\u{c}\n\r\t\u{b}'\"\\\\z");
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    let original = r#"C:\Fixtures\O'Neil\$unexpanded\"literal"#;
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", original, None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        original
    );
}

#[test]
fn writing_an_accepted_bare_account_key_replaces_it_instead_of_creating_duplicates() {
    for text in [
        "\u{feff}TELEGRAM_SESSION_NAME_WORK\nOTHER=kept\n",
        "export TELEGRAM_SESSION_NAME_WORK\nOTHER=kept\n",
    ] {
        let (dir, _state) = fixture();
        let path = dir.path().join(".env");
        std::fs::write(&path, text).unwrap();
        write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "work", None).unwrap();
        let parsed = read_config(&path).unwrap();
        assert_eq!(parsed.values["TELEGRAM_SESSION_NAME_WORK"], "work");
        assert_eq!(parsed.values["OTHER"], "kept");
        assert_eq!(
            std::fs::read_to_string(path)
                .unwrap()
                .matches("TELEGRAM_SESSION_NAME_WORK")
                .count(),
            1
        );
    }
}

#[test]
fn accepted_source_removal_never_becomes_a_successful_empty_snapshot() {
    let (dir, _state) = fixture();
    let path = dir.path().join("selected.env");
    std::fs::write(&path, "TELEGRAM_API_ID=12345\n").unwrap();
    telegram_mcp::account_manager::select_source(&path).unwrap();
    telegram_mcp::account_manager::capture_process_config();
    assert_eq!(
        telegram_mcp::account_manager::current_config()
            .unwrap()
            .values["TELEGRAM_API_ID"],
        "12345"
    );
    std::fs::remove_file(&path).unwrap();
    assert!(telegram_mcp::account_manager::current_config().is_err());
    std::fs::write(&path, "OTHER=deliberately-empty-accounts\n").unwrap();
    assert_eq!(
        telegram_mcp::account_manager::current_config()
            .unwrap()
            .values["OTHER"],
        "deliberately-empty-accounts"
    );
}

#[test]
fn backup_retention_is_per_selected_source_and_unrelated_backup_directories_are_ignored() {
    let (dir, _state) = fixture();
    let first = dir.path().join("first.env");
    let second = dir.path().join("second.env");
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "one", None).unwrap();
    write_env_value(&second, "TELEGRAM_SESSION_NAME_WORK", "other", None).unwrap();
    let trap = dir.path().join("first.env.backup-000000");
    std::fs::create_dir(&trap).unwrap();
    std::fs::write(trap.join("preserve"), "directory").unwrap();
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "two", None).unwrap();
    write_env_value(&second, "TELEGRAM_SESSION_NAME_WORK", "different", None).unwrap();
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "three", None).unwrap();
    for prefix in ["first.env.backup-", "second.env.backup-"] {
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().unwrap().is_file()
                    && entry.file_name().to_string_lossy().starts_with(prefix))
                .count(),
            1
        );
    }
    assert_eq!(
        read_config(&first).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "three"
    );
    assert!(trap.join("preserve").exists());
}

#[tokio::test]
async fn native_configuration_writer_respects_the_launchers_shared_byte_lock() {
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    if let Some(path) = std::env::var_os("ACCOUNT_REVIEW_LOCK_CHILD") {
        use std::io::{Read, Write};
        if std::env::var("ACCOUNT_REVIEW_HOLDER_PROBE").as_deref() == Ok("noisy") {
            std::io::stderr().write_all(&vec![b'n'; 131072]).unwrap();
            std::io::stderr().flush().unwrap();
        }
        println!("READY");
        std::io::stdout().flush().unwrap();
        if std::env::var("ACCOUNT_REVIEW_HOLDER_PROBE").as_deref() == Ok("exit") {
            eprintln!("Synthetic holder exited before acquiring the lock.");
            std::process::exit(19);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: fcntl reads this initialized record for a live descriptor.
            let mut region: libc::flock = unsafe { std::mem::zeroed() };
            region.l_type = libc::F_WRLCK as _;
            region.l_whence = libc::SEEK_SET as _;
            region.l_len = 1;
            assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &region) }, 0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::LockFile;
            // SAFETY: the open file remains live while its first byte is locked.
            let locked = unsafe { LockFile(file.as_raw_handle(), 0, 0, 1, 0) };
            assert_ne!(locked, 0, "{}", std::io::Error::last_os_error());
        }
        println!("LOCKED");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        return;
    }
    let (dir, _state) = fixture();
    let path = dir.path().join("selected.env");
    std::fs::write(&path, "TELEGRAM_SESSION_NAME_WORK=original\n").unwrap();
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    // Use the same OS byte-range lock as FileStream.Lock, without starting PowerShell.
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "native_configuration_writer_respects_the_launchers_shared_byte_lock",
            "--nocapture",
        ])
        .env("ACCOUNT_REVIEW_LOCK_CHILD", &lock)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let diagnostics = tokio::spawn(async move {
        let mut retained = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let keep = count.min(4096 - retained.len());
                    retained.extend_from_slice(&chunk[..keep]);
                    // Continue draining after the diagnostic limit: an unread
                    // stderr pipe must not prevent the holder announcing READY.
                }
            }
        }
        retained
    });
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let handshake = async {
        // Keep process startup separate from the byte-range lock handshake.
        for (phase, budget) in [("READY", 30), ("LOCKED", 10)] {
            tokio::time::timeout(Duration::from_secs(budget), async {
                loop {
                    let line = lines
                        .next_line()
                        .await
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Lock holder exited before {phase}"))?;
                    if line == phase {
                        return Ok::<(), String>(());
                    }
                }
            })
            .await
            .map_err(|_| format!("Lock holder did not announce {phase} within {budget}s"))??;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = handshake {
        let mut status = child.try_wait().unwrap();
        if status.is_none() {
            let _ = child.start_kill();
            if let Ok(Ok(exited)) = tokio::time::timeout(Duration::from_secs(5), child.wait()).await
            {
                status = Some(exited);
            }
        }
        let stderr = tokio::time::timeout(Duration::from_secs(2), diagnostics)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        panic!(
            "{error}; child status: {status:?}; bounded holder stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
    }
    assert!(write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "replacement", None).is_err());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("=original")
    );
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"\n")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    diagnostics.await.unwrap();
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "replacement", None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "replacement"
    );
}
