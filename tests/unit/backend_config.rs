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
        Sha256::digest(source_text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(row["native_session"], false);
    assert_eq!(row["source"], "configuration");
    assert_eq!(row["backend"], "grammers");
    assert_eq!(row["secret_chat_available"], true);
    assert_eq!(output["secret_chat_available"], true);
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
        Sha256::digest(source_text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
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

#[tokio::test]
async fn opening_a_removed_configuration_never_touches_its_private_authorization() {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = std::sync::Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    let path = state.root().join("retired.session.json");
    // A deliberately opaque retained record is a failsafe: even if the source
    // guard regresses, parsing fails before any transport can be constructed.
    std::fs::write(&path, b"retained-authorization-canary").unwrap();
    let snapshot = crate::account_manager::ConfigSnapshot {
        values: BTreeMap::from([("TELEGRAM_API_ID".into(), "1".into())]),
        digest: String::new(),
        path: temp.path().join(".env"),
        process_keys: BTreeSet::new(),
    };
    let error = crate::backend::Native::open_snapshot(
        state.clone(),
        "retired",
        "retired",
        &snapshot,
        false,
    )
    .await
    .err()
    .expect("An unconfigured authorization cannot be attached");
    assert!(error.to_string().contains("no longer configured"));
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"retained-authorization-canary"
    );
    assert!(!state.root().join("retired.session.lock").exists());
}

fn pool_string(key: u8) -> String {
    let mut bytes = vec![2, 127, 0, 0, 1, 1, 187];
    bytes.extend([key; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}

fn pool_state() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    (temp, state)
}

#[test]
fn pooled_configuration_reports_default_and_tracks_the_entire_pool_revision() {
    let (temp, state) = pool_state();
    let first = pool_string(0x41);
    let second = pool_string(0x42);
    let raw = format!(" ; {first},\n{second}; ");
    let input = snapshot(&[("TELEGRAM_SESSION_STRINGS", &raw)]);
    assert!(
        source_from(&input, "default") == Some(("TELEGRAM_SESSION_STRINGS".into(), raw.clone())),
        "The public pool revision must cover every configured token"
    );
    let pool = pooled_sessions(&input);
    assert_eq!(pool.len(), 2);
    std::fs::write(
        state
            .root()
            .join(format!("{}.session.json", pool[1].slot_label)),
        b"{}",
    )
    .unwrap();
    let report = serde_json::to_value(configuration_report(&input, state.root()).unwrap()).unwrap();
    let rows = report["accounts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["label"], "default");
    assert_eq!(rows[0]["config_key"], "TELEGRAM_SESSION_STRINGS");
    assert_eq!(
        rows[0]["config_digest"],
        crate::account_manager::config_digest(&raw)
    );
    assert_eq!(rows[0]["native_session"], true);
    for private in [&first, &second, &temp.path().display().to_string()] {
        assert!(!report.to_string().contains(private));
    }
}

#[tokio::test]
async fn pooled_slots_keep_secret_custody_separate_and_reuse_their_authorization_binding() {
    use crate::session::PersistentSession;
    use grammers_session::{Session, types::PeerInfo};
    let (_temp, state) = pool_state();
    let first_value = pool_string(0x43);
    let second_value = pool_string(0x44);
    let raw = format!("{first_value};{second_value}");
    let input = snapshot(&[
        ("TELEGRAM_SESSION_STRINGS", &raw),
        ("TELEGRAM_LOCK_GRACE_SECONDS", "0"),
    ]);
    let (first, first_source, first_slot) =
        crate::backend::startup::open_leased_session(&state, "default", &input, false)
            .await
            .unwrap();
    assert_eq!(first_source.as_ref().unwrap().1, first_value);
    let binding = format!(
        "TELEGRAM_SESSION_STRINGS:{}",
        crate::account_manager::config_digest(&first_value)
    );
    first.bind_source(Some(binding.clone())).unwrap();
    first
        .cache_peer(PeerInfo::User {
            id: 71,
            auth: None,
            bot: Some(false),
            is_self: Some(true),
        })
        .await
        .unwrap();
    let prepared = crate::secret::SecretRuntime::prepare(Arc::clone(&state), "default", &first)
        .await
        .unwrap()
        .unwrap();
    drop(prepared);
    let (second, second_source, second_slot) =
        crate::backend::startup::open_leased_session(&state, "default", &input, false)
            .await
            .unwrap();
    assert_ne!(first_slot, second_slot);
    assert_eq!(second_source.as_ref().unwrap().1, second_value);
    second
        .cache_peer(PeerInfo::User {
            id: 72,
            auth: None,
            bot: Some(false),
            is_self: Some(true),
        })
        .await
        .unwrap();
    let prepared = crate::secret::SecretRuntime::prepare(Arc::clone(&state), "default", &second)
        .await
        .unwrap()
        .unwrap();
    drop(prepared);
    let directory = state.root().join("secret-chats");
    for slot in [&first_slot, &second_slot] {
        assert!(directory.join(format!("{slot}.v1.enc")).is_file());
    }
    assert!(!directory.join("default.v1.enc").exists());
    drop(first);
    drop(second);
    let reopened = PersistentSession::open(Arc::clone(&state), &first_slot).unwrap();
    reopened.bind_source(Some(binding)).unwrap();
    assert_eq!(
        reopened
            .peer(grammers_session::types::PeerId::self_user())
            .await
            .unwrap()
            .unwrap()
            .id()
            .bare_id(),
        Some(71)
    );
}

#[tokio::test]
async fn pooled_custody_refuses_another_labels_authorization_and_allows_distinct_keys() {
    use crate::session::PersistentSession;
    for owner in ["old", "default"] {
        for same in [true, false] {
            let (_temp, state) = pool_state();
            let original = PersistentSession::open(Arc::clone(&state), owner).unwrap();
            original.import_telethon(&pool_string(0x45)).unwrap();
            drop(original);
            let directory = state.root().join("secret-chats");
            std::fs::create_dir(&directory).unwrap();
            let custody = directory.join(format!("{owner}.v1.enc"));
            std::fs::write(&custody, b"retained-custody-canary").unwrap();
            let own_key = if owner == "default" {
                "TELEGRAM_SESSION_NAME"
            } else {
                "TELEGRAM_SESSION_NAME_OLD"
            };
            let own = snapshot(&[(own_key, owner), ("TELEGRAM_LOCK_GRACE_SECONDS", "0")]);
            drop(
                crate::backend::startup::open_leased_session(&state, owner, &own, false)
                    .await
                    .unwrap(),
            );
            let value = pool_string(if same { 0x45 } else { 0x46 });
            let input = snapshot(&[
                ("TELEGRAM_SESSION_STRINGS", &value),
                ("TELEGRAM_LOCK_GRACE_SECONDS", "0"),
            ]);
            let slot = &pooled_sessions(&input)[0].slot_label;
            let result =
                crate::backend::startup::open_leased_session(&state, "default", &input, false)
                    .await;
            if same {
                let error = result
                    .err()
                    .expect("A pool alias must not split another label's custody");
                assert!(error.to_string().contains("existing secret-chat custody"));
                assert!(!error.to_string().contains(&value));
                assert!(!state.root().join(format!("{slot}.session.json")).exists());
            } else {
                drop(result.unwrap());
            }
            assert_eq!(std::fs::read(custody).unwrap(), b"retained-custody-canary");
        }
    }
}

#[tokio::test]
async fn pooled_invalid_tokens_are_refused_before_opening_any_slot() {
    let first = pool_string(0x47);
    let second = pool_string(0x48);
    for raw in [
        format!("invalid-source-canary;{first};{second}"),
        format!("{first};invalid-source-canary;{second}"),
        format!("{first};{second};invalid-source-canary"),
    ] {
        let (_temp, state) = pool_state();
        let input = snapshot(&[
            ("TELEGRAM_SESSION_STRINGS", &raw),
            ("TELEGRAM_LOCK_GRACE_SECONDS", "0"),
        ]);
        let error = crate::backend::startup::open_leased_session(&state, "default", &input, false)
            .await
            .err()
            .expect("Invalid pool tokens must fail validation");
        assert!(!format!("{error:#}").contains("invalid-source-canary"));
        assert!(
            !std::fs::read_dir(state.root())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("pool_"))
        );
    }
}

#[tokio::test]
async fn pooled_health_recognizes_the_full_configuration_without_opening_a_session() {
    let (temp, state) = pool_state();
    let raw = format!("{};{}", pool_string(0x49), pool_string(0x4a));
    let path = temp.path().join("pool.env");
    std::fs::write(&path, format!("TELEGRAM_SESSION_STRINGS='{raw}'\n")).unwrap();
    let result = crate::account_manager::session_health_with_state(
        &state,
        &path,
        "TELEGRAM_SESSION_STRINGS",
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "unknown");
    assert_eq!(
        result["config_digest"],
        crate::account_manager::config_digest(&raw)
    );
    assert!(
        !std::fs::read_dir(state.root())
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("pool_"))
    );
}

#[tokio::test]
async fn pooled_first_import_skips_an_authorization_already_leased_by_another_label() {
    let (_temp, state) = pool_state();
    let first = pool_string(0x4b);
    let second = pool_string(0x4c);
    let owner = crate::session::PersistentSession::open(Arc::clone(&state), "active").unwrap();
    owner.import_telethon(&first).unwrap();
    let raw = format!("{first};{second}");
    let input = snapshot(&[
        ("TELEGRAM_SESSION_STRINGS", &raw),
        ("TELEGRAM_LOCK_GRACE_SECONDS", "0"),
    ]);
    let (session, source, slot) =
        crate::backend::startup::open_leased_session(&state, "default", &input, false)
            .await
            .unwrap();
    assert_eq!(source.as_ref().unwrap().1, second);
    assert_eq!(slot, pooled_sessions(&input)[1].slot_label);
    drop(session);
    drop(owner);
}

#[tokio::test]
async fn pooled_legacy_default_custody_requires_verified_explicit_owner() {
    use crate::{secret::SecretHistory, session::PersistentSession};
    use grammers_session::{Session, types::PeerInfo};
    use kiaro_secret_chat::{
        chat::{Chat, ChatState},
        crypto::SharedKey,
        storage::{SecretRecord, Storage},
    };

    let (temp, state) = pool_state();
    let original = pool_string(0x4d);
    let other = pool_string(0x4e);
    let raw = format!("{original};{other}");
    let input = snapshot(&[
        ("TELEGRAM_SESSION_STRINGS", &raw),
        ("TELEGRAM_LOCK_GRACE_SECONDS", "0"),
    ]);
    let slot = pooled_sessions(&input)[0].slot_label.clone();
    let original_session = PersistentSession::open(Arc::clone(&state), &slot).unwrap();
    original_session.import_telethon(&original).unwrap();
    original_session
        .bind_source(Some(format!(
            "TELEGRAM_SESSION_STRINGS:{}",
            crate::account_manager::config_digest(&original)
        )))
        .unwrap();
    let verified_self = PeerInfo::User {
        id: 73,
        auth: None,
        bot: Some(false),
        is_self: Some(true),
    };
    original_session
        .cache_peer(verified_self.clone())
        .await
        .unwrap();
    drop(original_session);

    let binding = crate::secret::session_binding(73, 2, &[0x4d; 256]).unwrap();
    let mut old_store = crate::secret::open_store(state.root(), "default", &binding, true)
        .unwrap()
        .unwrap();
    let mut saved = old_store.load().unwrap().unwrap();
    let mut chat = Chat::new(77, 123, 74, true, ChatState::Ready);
    chat.key = Some(SharedKey::from_slice(&[0x4f; 256]).unwrap());
    saved.chats.insert(77, chat);
    saved.private_records.insert(
        "ingress/journal-v1".into(),
        SecretRecord::new(
            serde_json::to_vec(
                &json!({"version":1,"checkpoint":null,"next_id":1,"batches":[],"paused":false}),
            )
            .unwrap(),
        ),
    );
    old_store.commit(&saved).unwrap();
    drop(old_store);
    let directory = state.root().join("secret-chats");
    let history = SecretHistory::open(&directory, "default", &BTreeMap::new()).unwrap();
    let row =
        json!({"message_id":81,"text":"retained private fixture","date":"2026-01-01T00:00:00Z"});
    history.append(77, row.clone(), None).unwrap();
    drop(history);
    let preserved = [
        directory.join("default.v1.enc"),
        directory.join("default.store.key"),
        directory.join("default.history.v1.json"),
        state.root().join(format!("{slot}.session.json")),
    ]
    .map(|path| {
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    });
    let error = crate::backend::startup::open_leased_session(&state, "default", &input, false)
        .await
        .err()
        .expect("A legacy default store without an identified owner must be preserved");
    assert!(error.to_string().contains("Cannot identify an existing"));
    assert!(!state.root().join("default.session.json").exists());
    for (path, bytes) in &preserved {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
    }

    let env = temp.path().join("recovery.env");
    std::fs::write(
        &env,
        format!("TELEGRAM_SESSION_STRINGS='{raw}'\nTELEGRAM_LOCK_GRACE_SECONDS=0\n"),
    )
    .unwrap();
    crate::account_manager::write_env_value(&env, "TELEGRAM_SESSION_STRING", &original, None)
        .unwrap();
    let selected = crate::account_manager::read_config(&env).unwrap();
    assert!(selected.values.contains_key("TELEGRAM_SESSION_STRINGS"));
    let (session, source, leased) =
        crate::backend::startup::open_leased_session(&state, "default", &selected, false)
            .await
            .unwrap();
    assert_eq!(leased, "default");
    assert_eq!(source.as_ref().unwrap().0, "TELEGRAM_SESSION_STRING");
    // This offline fixture supplies the verified self result that live startup obtains first.
    session.cache_peer(verified_self).await.unwrap();
    crate::session::check_deferred_custody_with_config(&state, "default", &selected).unwrap();
    let prepared = crate::secret::SecretRuntime::prepare(Arc::clone(&state), "default", &session)
        .await
        .unwrap()
        .unwrap();
    drop(prepared);
    let reopened = crate::secret::open_store(state.root(), "default", &binding, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened.load().unwrap().unwrap().chats[&77].state,
        ChatState::Ready
    );
    drop(reopened);
    let history = SecretHistory::open(&directory, "default", &BTreeMap::new()).unwrap();
    assert_eq!(history.read(77, 10).unwrap(), vec![row]);
    for (path, bytes) in &preserved {
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
    }
    assert!(!directory.join(format!("{slot}.v1.enc")).exists());
}
