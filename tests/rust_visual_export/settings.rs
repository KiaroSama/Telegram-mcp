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
    assert!(parsed.path.starts_with(&destination));
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
