use super::*;

fn snapshot(values: &[(&str, &str)]) -> crate::account_manager::ConfigSnapshot {
    crate::account_manager::ConfigSnapshot {
        values: values
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        digest: "private-file-revision-not-public".into(),
        path: "private/configuration/.env".into(),
        process_keys: BTreeSet::new(),
    }
}
#[test]
fn public_configuration_projection_preserves_contract_without_credential_or_unrelated_fields() {
    let directory = tempfile::tempdir().unwrap();
    let source_text = "synthetic-private-session-canary-قدیمی";
    let api_hash = "synthetic-api-hash-canary-never-public";
    let input = snapshot(&[
        ("TELEGRAM_SESSION_STRING_WORK", source_text),
        ("TELEGRAM_API_HASH", api_hash),
        ("TELEGRAM_API_ID", "12345"),
        (
            "TELEGRAM_SESSION_STRING___LOGIN_STAGE",
            "private-staging-canary",
        ),
    ]);
    let output =
        serde_json::to_value(configuration_report(&input, directory.path()).unwrap()).unwrap();
    let rows = output["accounts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["label"], "work");
    assert_eq!(row["account"], "work");
    assert_eq!(row["config_key"], "TELEGRAM_SESSION_STRING_WORK");
    assert_eq!(
        row["config_digest"],
        format!("{:x}", Sha256::digest(source_text.as_bytes()))
    );
    assert_eq!(row["native_session"], false);
    assert_eq!(row["source"], "configuration");
    assert_eq!(row["backend"], "grammers");
    assert_eq!(row["secret_chat_available"], false);
    assert_eq!(row.as_object().unwrap().len(), 8);
    for canary in [
        source_text,
        api_hash,
        "private-staging-canary",
        "private-file-revision-not-public",
        "private/configuration",
    ] {
        assert!(!output.to_string().contains(canary));
    }
}
#[test]
fn source_preference_and_unicode_whitespace_digest_match_native_binding() {
    let directory = tempfile::tempdir().unwrap();
    let source_text = "نام حساب  ";
    let input = snapshot(&[
        ("TELEGRAM_SESSION_NAME_WORK", "ignored-name-canary"),
        ("TELEGRAM_SESSION_STRING_WORK", source_text),
        ("TELEGRAM_SESSION_NAME", "default-name"),
        ("TELEGRAM_SESSION_NAME_EMPTY", ""),
    ]);
    let output =
        serde_json::to_value(configuration_report(&input, directory.path()).unwrap()).unwrap();
    assert_eq!(output["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(output["accounts"][0]["label"], "default");
    assert_eq!(output["accounts"][1]["label"], "work");
    assert_eq!(
        output["accounts"][1]["config_digest"],
        format!("{:x}", Sha256::digest(source_text.as_bytes()))
    );
    assert_eq!(
        output["accounts"][1]["config_key"],
        "TELEGRAM_SESSION_STRING_WORK"
    );
}
#[test]
fn public_native_state_flag_checks_only_the_selected_validated_label() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("work.session.json"), b"{}").unwrap();
    let input = snapshot(&[
        ("TELEGRAM_SESSION_NAME_WORK", "work-source"),
        ("TELEGRAM_SESSION_NAME_OTHER", "other-source"),
    ]);
    let output =
        serde_json::to_value(configuration_report(&input, directory.path()).unwrap()).unwrap();
    assert_eq!(output["accounts"][0]["native_session"], false);
    assert_eq!(output["accounts"][1]["native_session"], true);
}
#[test]
fn invalid_configuration_labels_fail_without_echoing_source_values() {
    let directory = tempfile::tempdir().unwrap();
    let canary = "never-echo-this-source-canary";
    for key in [
        "TELEGRAM_SESSION_STRING_".to_owned(),
        "TELEGRAM_SESSION_NAME_BAD-LABEL".to_owned(),
        format!("TELEGRAM_SESSION_NAME_{}", "X".repeat(65)),
    ] {
        let input = snapshot(&[(&key, canary)]);
        let error = match configuration_report(&input, directory.path()) {
            Ok(_) => panic!("Invalid label was accepted"),
            Err(error) => error,
        };
        assert!(!format!("{error:#}").contains(canary));
    }
}

#[test]
fn public_revision_serialization_keeps_all_digest_bytes_including_leading_zeroes() {
    assert_eq!(
        serde_json::to_value(ConfigurationRevision([0; 32])).unwrap(),
        json!("0".repeat(64))
    );
    assert_eq!(
        serde_json::to_value(ConfigurationRevision([0xab; 32])).unwrap(),
        json!("ab".repeat(32))
    );
}
