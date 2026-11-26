use super::*;
use chrono::TimeZone;

fn arguments(destination: &std::path::Path) -> Value {
    json!({"format":"html_and_json","photos":true,"videos":false,"voice_messages":true,"video_messages":false,"stickers":true,"gifs":false,"files":true,"size_limit_mb":1.25,"date_from":"2023-01-02 03:04","date_to":"2023-01-03 03:04","destination":destination})
}

#[test]
fn valid_export_settings_keep_media_choices_local_dates_fractional_budget_and_unused_path() {
    if isolated(
        "settings_edges::valid_export_settings_keep_media_choices_local_dates_fractional_budget_and_unused_path",
    ) {
        return;
    }
    let destination = std::path::PathBuf::from(std::env::var("TEST_EXPORT_ROOT").unwrap());
    let empty_target = destination.join("empty-selected");
    let empty_parsed = Settings::parse(&arguments(&empty_target)).unwrap();
    assert_eq!(
        empty_parsed.path,
        destination.canonicalize().unwrap().join("empty-selected")
    );
    assert!(
        !empty_parsed.path.exists(),
        "Planning an empty explicit destination must not touch disk"
    );

    std::fs::write(destination.join("existing.txt"), b"occupied").unwrap();
    let args = arguments(&destination);
    let parsed = Settings::parse(&args).unwrap();
    assert_eq!(parsed.media, [true, false, true, false, true, false, true]);
    assert_eq!(parsed.max_bytes, 1310720);
    assert_eq!(
        parsed.from,
        chrono::Local
            .with_ymd_and_hms(2023, 1, 2, 3, 4, 0)
            .single()
            .unwrap()
            .timestamp()
    );
    assert!(!parsed.includes_date(parsed.from - 1));
    assert!(parsed.includes_date(parsed.from));
    assert!(parsed.includes_date(parsed.until - 1));
    assert!(!parsed.includes_date(parsed.until));
    assert!(parsed.path.starts_with(destination.canonicalize().unwrap()));
    assert_ne!(parsed.path, destination.canonicalize().unwrap());
    assert!(!parsed.path.exists());
    std::fs::create_dir(&parsed.path).unwrap();
    let occupied = parsed.path.with_file_name(format!(
        "{}_1",
        parsed.path.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir(occupied).unwrap();
    let next = Settings::parse(&args).unwrap();
    assert!(
        next.path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_2")
    );
    assert!(
        !next.path.exists(),
        "Planning an export must not create or overwrite its destination"
    );
}

#[test]
fn unspecified_destination_uses_allowed_downloads_without_creating_a_directory() {
    if isolated(
        "settings_edges::unspecified_destination_uses_allowed_downloads_without_creating_a_directory",
    ) {
        return;
    }
    let mut args = arguments(std::path::Path::new("unused"));
    args.as_object_mut().unwrap().remove("destination");
    let parsed = Settings::parse(&args).unwrap();
    let allowed = telegram_mcp::state::State::from_environment()
        .unwrap()
        .default_downloads()
        .unwrap();
    assert!(parsed.path.starts_with(allowed));
    assert!(!parsed.path.exists());
}

#[derive(Clone, Default)]
struct CleanupFixture {
    inner: Fixture,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Telegram for CleanupFixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        if method == "invokeWithTakeout" && params["query"]["_"] == "account.finishTakeoutSession" {
            self.entered.notify_one();
            self.release.notified().await;
            return Ok(json!(true));
        }
        self.inner.invoke(method, params).await
    }
    async fn resolve_peer(&self, peer: &Value) -> Result<Value> {
        self.inner.resolve_peer(peer).await
    }
}

#[test]
fn cancellation_during_takeout_cleanup_preserves_completed_export_success() {
    if isolated(
        "settings_edges::cancellation_during_takeout_cleanup_preserves_completed_export_success",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = CleanupFixture::default();
        let entered = api.entered.clone();
        let release = api.release.clone();
        let settings = settings("cleanup-cancel");
        let output = settings.path.join("result.json");
        let job = export::jobs::start(Arc::new(api), "chat", json!({"chat_id":"@test"}), settings)
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(saved["messages"].as_array().unwrap().len(), 103);
        assert_eq!(
            export::jobs::cancel(&json!({"job_id":job["job_id"]})).unwrap()["cancelled"],
            true
        );
        release.notify_one();
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let status = export::jobs::status(&json!({"job_id":job["job_id"]})).unwrap();
                if status["status"] != "running" {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(status["status"], "done");
        assert_eq!(status["result"]["messages"], 103);
        assert!(status.get("error").is_none());
    });
}

#[test]
fn concurrent_job_admission_reserves_distinct_folders_and_rolls_back_cancelled_empty_ones() {
    if isolated(
        "settings_edges::concurrent_job_admission_reserves_distinct_folders_and_rolls_back_cancelled_empty_ones",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let destination = std::path::PathBuf::from(std::env::var("TEST_EXPORT_ROOT").unwrap());
        std::fs::write(destination.join("existing-root.txt"), b"keep").unwrap();
        let args = arguments(&destination);
        let first = Settings::parse(&args).unwrap();
        let second = Settings::parse(&args).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
        assert_eq!(
            first.path, second.path,
            "Pure planning should not mutate the filesystem"
        );
        let occupied = first.path.clone();
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("owner.txt"), b"earlier owner export").unwrap();
        let api = Fixture {
            block: true,
            ..Default::default()
        };
        let first = export::jobs::start(
            Arc::new(api.clone()),
            "chat",
            json!({"chat_id":"@test"}),
            first,
        )
        .unwrap();
        let second =
            export::jobs::start(Arc::new(api), "chat", json!({"chat_id":"@test"}), second).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&destination)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o750,
                "Reserving a child must not change the owner's destination permissions"
            );
        }
        assert_ne!(
            first["folder"], second["folder"],
            "Concurrent admitted jobs must own different output folders"
        );
        for job in [first, second] {
            let folder = std::path::Path::new(job["folder"].as_str().unwrap());
            assert!(
                folder.is_dir(),
                "A running job must already own its reserved folder"
            );
            assert_eq!(
                export::jobs::cancel(&json!({"job_id":job["job_id"]})).unwrap()["cancelled"],
                true
            );
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while export::jobs::status(&json!({"job_id":job["job_id"]})).unwrap()["status"]
                    == "running"
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !folder.exists(),
                "Cancelling before output must roll back only the empty owned reservation"
            );
        }
        assert_eq!(
            std::fs::read(occupied.join("owner.txt")).unwrap(),
            b"earlier owner export"
        );
    });
}

#[test]
fn export_settings_refuse_reversed_dates_and_invalid_numeric_budgets_before_creating_files() {
    let mut args = arguments(std::path::Path::new("unused"));
    args["date_to"] = json!("2023-01-01");
    assert!(
        Settings::parse(&args)
            .unwrap_err()
            .to_string()
            .contains("later than")
    );
    args["date_to"] = json!("2023-01-02 03:04");
    assert!(Settings::parse(&args).is_err());
    args["date_to"] = json!("2023-01-03");
    for budget in [json!(0.5), json!(4000.1), json!("1"), Value::Null] {
        args["size_limit_mb"] = budget;
        assert!(
            Settings::parse(&args)
                .unwrap_err()
                .to_string()
                .contains("size_limit_mb")
        );
    }
}

#[test]
fn export_dates_use_local_midnight_and_refuse_impossible_calendar_values() {
    assert_eq!(telegram_mcp::export::settings::parse_date("").unwrap(), 0);
    assert_eq!(
        telegram_mcp::export::settings::parse_date("2024-02-29").unwrap(),
        chrono::Local
            .with_ymd_and_hms(2024, 2, 29, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp()
    );
    for date in ["2023-02-29", "2024-13-01", "2024-01-01 25:00", "01/01/2024"] {
        assert!(
            telegram_mcp::export::settings::parse_date(date)
                .unwrap_err()
                .to_string()
                .contains("Export dates")
        );
    }
}
