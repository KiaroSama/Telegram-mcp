use super::*;
use telegram_mcp::account_manager::{read_config, session_validate};

const CONFIG: &str = "# Keep this: café\r\nTELEGRAM_STATE_DIR='local state'\r\nTELEGRAM_SESSION_NAME_WORK=native\r\n# TELEGRAM_SESSION_STRING_OLD=inactive\r\nTELEGRAM_SESSION_STRING=invalid-fixture\r\nUNRELATED_SETTING='keep me'\r\n";

fn fixture() -> Checkout {
    let checkout = Checkout::new();
    fs::write(checkout.env_file(), CONFIG).unwrap();
    checkout
}

#[test]
fn menu_lists_file_owned_accounts_in_order_and_cancels_without_backing_up() {
    let checkout = fixture();
    let (text, _) = success(checkout.native(&["manage"], "1\n3\n0\n3\n1\nn\nexit\n"));
    assert!(text.contains("Configured accounts (2)"));
    assert!(text.find("1. work").unwrap() < text.find("2. default").unwrap());
    assert!(!text.contains("invalid-fixture"));
    assert!(!text.contains("inactive"));
    assert!(text.matches("Cancelled.").count() >= 2);
    assert!(text.contains("Selection {quit=exit}"));
    assert!(text.contains("{back=0, quit=exit}"));
    assert_eq!(fs::read_to_string(checkout.env_file()).unwrap(), CONFIG);
    assert!(checkout.files(".", ".env.backup-").is_empty());
}

#[test]
fn first_use_respects_decline_eof_and_acceptance() {
    let checkout = Checkout::new();
    success(checkout.native(&["account-manager"], "n\n"));
    success(checkout.native(&["manage"], ""));
    assert!(!checkout.env_file().exists());
    success(checkout.native(&["manage"], "\nexit\n"));
    assert_eq!(fs::read(checkout.env_file()).unwrap(), b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(checkout.env_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

fn legacy_session(byte: u8) -> String {
    use base64::Engine;
    let mut bytes = vec![byte; 263];
    bytes[0] = 2;
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}

#[test]
fn add_normalizes_a_label_and_replaces_its_exact_key_with_one_private_backup() {
    let checkout = fixture();
    let first = legacy_session(1);
    let second = legacy_session(2);
    assert_eq!(session_validate(&first), "valid");
    let (text, stderr) = success(checkout.native(
        &["manage"],
        &format!(
            "2\nA @ B\n  KGB - Verifier  \nn\n{first}\n2\nKGB Verifier\n\nn\n{second}\nexit\n"
        ),
    ));
    assert!(text.contains("Stored as 'kgb_verifier'"));
    assert!(text.contains("Added 'kgb_verifier'"));
    for session in [&first, &second] {
        assert!(!text.contains(session));
        assert!(!stderr.contains(session));
    }
    let contents = fs::read_to_string(checkout.env_file()).unwrap();
    assert!(contents.starts_with(CONFIG));
    assert_eq!(
        contents
            .matches("TELEGRAM_SESSION_STRING_KGB_VERIFIER=")
            .count(),
        1
    );
    let values = read_config(&checkout.env_file()).unwrap().values;
    assert_eq!(values["TELEGRAM_SESSION_STRING_KGB_VERIFIER"], second);
    let backup = checkout.files(".", ".env.backup-");
    assert_eq!(backup.len(), 1);
    assert!(fs::read_to_string(&backup[0]).unwrap().contains(&first));
    assert!(checkout.files(".", ".tmp").is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [checkout.env_file(), backup[0].clone()] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn malformed_import_is_never_saved_or_echoed() {
    let checkout = fixture();
    let invalid = "synthetic-invalid-session-value-do-not-echo";
    let (text, stderr) =
        success(checkout.native(&["manage"], &format!("2\nwork\n\nn\n{invalid}\nexit\n")));
    assert!(text.contains("cannot read that legacy session string"));
    assert!(!text.contains(invalid));
    assert!(!stderr.contains(invalid));
    assert_eq!(fs::read_to_string(checkout.env_file()).unwrap(), CONFIG);
    assert!(checkout.files(".", ".env.backup-").is_empty());
}

#[test]
fn rename_moves_file_owned_key_and_registry_section_without_a_second_config_write() {
    let checkout = fixture();
    let registry = checkout.root().join("secrets.md");
    fs::write(&registry, "## TELEGRAM_SESSION_NAME_WORK\r\naccount 'work'\r\nsample café\r\n## UNRELATED\r\naccount 'work'\r\n").unwrap();
    let (text, _) = success(checkout.native(&["manage"], "4\n999\nword\n1\nnew-label\n\nexit\n"));
    assert!(text.contains("Enter a number from 1 to 2"));
    assert!(text.contains("Renamed 'work' to 'new_label'"));
    let snapshot = read_config(&checkout.env_file()).unwrap();
    assert_eq!(snapshot.values["TELEGRAM_SESSION_NAME_NEW_LABEL"], "native");
    assert!(!snapshot.values.contains_key("TELEGRAM_SESSION_NAME_WORK"));
    assert!(
        !snapshot
            .values
            .contains_key("TELEGRAM_SESSION_STRING_NEW_LABEL")
    );
    assert_eq!(snapshot.values["UNRELATED_SETTING"], "keep me");
    assert_eq!(
        fs::read_to_string(registry).unwrap(),
        "## TELEGRAM_SESSION_NAME_NEW_LABEL\r\naccount 'new_label'\r\nsample café\r\n## UNRELATED\r\naccount 'work'\r\n"
    );
    let backup = checkout.files(".", ".env.backup-");
    assert_eq!(backup.len(), 1);
    assert_eq!(fs::read_to_string(&backup[0]).unwrap(), CONFIG);
}

#[test]
fn rename_refuses_occupied_state_without_rewriting_configuration() {
    let checkout = fixture();
    let state = checkout.state().join("secret-chats");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("work-history.json"), b"source").unwrap();
    fs::write(state.join("occupied-history.json"), b"destination").unwrap();
    let (text, _) = success(checkout.native(&["manage"], "4\n1\noccupied\n\nexit\n"));
    assert!(text.contains("destination account already has stored files"));
    assert_eq!(fs::read_to_string(checkout.env_file()).unwrap(), CONFIG);
    assert_eq!(
        fs::read(state.join("work-history.json")).unwrap(),
        b"source"
    );
    assert_eq!(
        fs::read(state.join("occupied-history.json")).unwrap(),
        b"destination"
    );
    assert!(checkout.files(".", ".env.backup-").is_empty());
}

#[test]
fn remove_archives_only_the_selected_accounts_private_state() {
    let checkout = fixture();
    let state = checkout.state().join("secret-chats");
    fs::create_dir_all(state.join("work")).unwrap();
    for name in [
        "work-history.json",
        "work-media.json",
        "work.owner.json",
        "default-media.json",
    ] {
        fs::write(state.join(name), b"synthetic fixture").unwrap();
    }
    let (text, _) = success(checkout.native(&["manage"], "3\n1\n\nexit\n"));
    assert!(text.contains("Removed 'work'"));
    for name in [
        "work",
        "work-history.json",
        "work-media.json",
        "work.owner.json",
    ] {
        assert!(!state.join(name).exists());
    }
    assert!(state.join("default-media.json").exists());
    assert!(
        !read_config(&checkout.env_file())
            .unwrap()
            .values
            .contains_key("TELEGRAM_SESSION_NAME_WORK")
    );
    assert_eq!(checkout.files(".", ".env.backup-").len(), 1);
}

#[test]
fn health_menu_reports_status_and_cancels_without_invoking_login() {
    let checkout = fixture();
    let (text, _) = success(checkout.native(&["manage"], "6\n0\nexit\n"));
    assert!(text.contains("Checking current Telegram authorization"));
    assert!(text.contains("invalid"));
    assert!(text.contains("Number to re-login"));
    assert!(text.contains("Nothing was changed"));
    assert!(!text.contains("Phone number"));
    assert_eq!(fs::read_to_string(checkout.env_file()).unwrap(), CONFIG);
}
