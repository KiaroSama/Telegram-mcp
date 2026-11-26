use super::*;

#[tokio::test]
async fn transcribe_audio_checks_file_roots_without_entering_account_preflight() {
    const ROOT: &str = "TEST_LOCAL_TRANSCRIPTION_ROOT";
    let Some(root) = std::env::var_os(ROOT).map(std::path::PathBuf::from) else {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("test.env"),
            "TELEGRAM_SESSION_NAME_ONE=one\nTELEGRAM_SESSION_NAME_TWO=two\nTELEGRAM_TRANSCRIBE=off\n",
        )
        .unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TELEGRAM_") {
                command.env_remove(key);
            }
        }
        command
            .args([
                "--exact",
                "server::local_audio_tests::transcribe_audio_checks_file_roots_without_entering_account_preflight",
            ])
            .env(ROOT, temporary.path())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    crate::account_manager::select_source(&root.join("test.env")).unwrap();
    let files = root.join("files");
    std::fs::create_dir(&files).unwrap();
    let inside = files.join("inside.wav");
    let outside = root.join("outside.wav");
    for path in [&inside, &outside] {
        std::fs::write(path, b"fixture audio").unwrap();
    }
    let state = Arc::new(State::at(root.join("state"), vec![files]).unwrap());
    let server = Mcp {
        runtime: Arc::new(Runtime {
            accounts: Mutex::new(HashMap::new()),
            guard: Safeguard::new(Arc::clone(&state)),
            state,
            tools: catalogue::catalogue_for_mode("all").unwrap(),
            policy: access::AccessPolicy::default(),
        }),
    };
    let accounts = server.runtime.accounts.lock().await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        server.execute("transcribe_audio", json!({"file_path":inside})),
    )
    .await
    .expect("Standalone transcription cannot wait for the Telegram account registry")
    .unwrap();
    assert_eq!(result["reason"], "transcription_disabled");
    let pending = server
        .execute("transcribe_audio", json!({"file_path":outside}))
        .await
        .unwrap();
    assert_eq!(pending["performed"], false);
    assert_eq!(pending["reasons"], json!(["outside_project_folder"]));
    assert!(accounts.is_empty());
}
