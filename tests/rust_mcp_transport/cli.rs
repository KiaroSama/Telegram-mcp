use super::common::*;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn cli_help_and_version_need_no_config_credentials_or_state() {
    let fixture = Fixture::new();
    std::fs::write(fixture.cwd.join(".env"), [0xff, 0xfe]).unwrap();
    let help = cli(&fixture, &["--help"], None).await;
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("Native Rust Telegram MCP server"));
    assert!(text.contains("session-validate"));
    let version = cli(&fixture, &["--version"], None).await;
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("telegram-mcp {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(!fixture.state.exists());
}
#[tokio::test]
async fn session_validate_reads_a_piped_synthetic_legacy_session_without_authorizing() {
    let fixture = Fixture::new();
    let mut bytes = vec![2, 149, 154, 167, 51];
    bytes.extend(443_u16.to_be_bytes());
    bytes.extend([0x5a; 256]);
    let token = format!(
        "1{}\n",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    );
    let output = cli(&fixture, &["session-validate"], Some(token.as_bytes())).await;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "valid");
    assert!(output.stderr.is_empty());
    assert!(!fixture.state.exists());
}
#[tokio::test]
async fn session_validate_refuses_invalid_and_oversized_input_without_echoing_it() {
    let fixture = Fixture::new();
    for input in ["invalid private fixture input".to_owned(), "x".repeat(4097)] {
        let output = cli(&fixture, &["session-validate"], Some(input.as_bytes())).await;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "invalid");
        assert!(output.stderr.is_empty());
    }
    assert!(!fixture.state.exists());
}
#[tokio::test]
async fn login_from_pipes_refuses_before_creating_state_or_login_staging() {
    let fixture = Fixture::new();
    let before = std::fs::read(fixture.cwd.join(".env")).unwrap();
    let output = cli(&fixture, &["login", "--account", "work"], None).await;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("interactive terminal"));
    assert!(!fixture.state.exists());
    assert_eq!(std::fs::read(fixture.cwd.join(".env")).unwrap(), before);
}
async fn source_value(value: &str) {
    let fixture = Fixture::new();
    fixture.env_file(&format!("\u{feff}# Explicit UTF-8 account source\r\nTELEGRAM_SESSION_NAME_WORK=\"{value}\"\r\nUNRELATED=kept\r\n"));
    let before = std::fs::read(fixture.cwd.join(".env")).unwrap();
    let output = cli(&fixture, &["accounts", "--json"], None).await;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["accounts"].as_array().unwrap().len(), 1);
    let account = &result["accounts"][0];
    assert_eq!(account["label"], "work");
    assert_eq!(account["config_key"], "TELEGRAM_SESSION_NAME_WORK");
    assert_eq!(
        account["config_digest"],
        Sha256::digest(value.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(std::fs::read(fixture.cwd.join(".env")).unwrap(), before);
    fixture.assert_no_telegram_sessions();
}
#[tokio::test]
async fn ascii_environment_source_is_used_without_starting_telegram() {
    source_value("work_session").await;
}
#[tokio::test]
async fn unicode_environment_source_and_bom_crlf_round_trip_through_real_cli() {
    source_value("نام حساب  ").await;
}
