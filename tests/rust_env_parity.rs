//! Track T3 parity regressions: runtime env vars, safe downloads, proxy precedence,
//! lock grace, pooled sessions, live feed/alias stores, canonical ghost keys,
//! structuredContent, ordinary-choice elicitation, and preflight validation.
use base64::Engine;
use rmcp::model::Role;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use telegram_mcp::{
    account_manager::{
        ConfigSnapshot, DEFAULT_LOCK_GRACE_SECONDS, acquire_with_grace, parse_lock_grace_seconds,
    },
    backend::{ClientIdentity, pooled_sessions, session_auth_key_identity},
    permalinks::{parse_domain, strip_username_prefix},
    proxy,
    server::{
        catalogue,
        request_context::{
            RefusalOverrides, TrustedRequestContext, build_export_choice_schema,
            merge_elicited_choices, parse_tool_timeout, preflight_validate_tool,
        },
        results::{build_call_tool_result, roots_status_payload, user_audience_text},
    },
    state::{
        DANGEROUS_SUFFIXES, FileAccess, State, identifier_like, safe_suffix,
        sanitize_download_filename, sanitize_download_path, validate_path_syntax,
    },
    tools::secret::autosave::missing_export_choices,
};

fn snapshot(entries: &[(&str, &str)]) -> ConfigSnapshot {
    ConfigSnapshot {
        values: entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        digest: "test".into(),
        path: PathBuf::from(".env"),
        process_keys: BTreeSet::new(),
    }
}

fn legacy_string_session(dc: u8, ip: [u8; 4], port: u16, key_byte: u8) -> String {
    let mut raw = Vec::with_capacity(263);
    raw.push(dc);
    raw.extend_from_slice(&ip);
    raw.extend_from_slice(&port.to_be_bytes());
    raw.extend_from_slice(&[key_byte; 256]);
    format!("1{}", base64::engine::general_purpose::URL_SAFE.encode(raw))
}

#[test]
fn safe_suffix_and_download_paths_enforce_denylist_windows_normalization_and_syntax() {
    for blocked in DANGEROUS_SUFFIXES {
        assert_eq!(safe_suffix(blocked), ".bin", "blocked {blocked}");
        assert_eq!(
            safe_suffix(&blocked.to_ascii_uppercase()),
            ".bin",
            "uppercase blocked {blocked}"
        );
        let name = format!("payload{blocked}. .");
        assert_eq!(
            sanitize_download_filename(&name).unwrap(),
            "payload.bin",
            "windows trailing normalization for {name}"
        );
    }
    assert_eq!(safe_suffix(".PNG"), ".PNG");
    assert_eq!(safe_suffix(".PDF"), ".PDF");
    assert_eq!(safe_suffix(".tar"), ".tar");
    assert_eq!(safe_suffix(".1234567"), ".1234567");
    assert_eq!(safe_suffix(".12345678"), ".bin");
    assert_eq!(safe_suffix(".bad-ext"), ".bin");
    assert_eq!(safe_suffix(""), ".bin");

    let (replaced, old_ext) =
        telegram_mcp::state::target_path(std::path::Path::new("notes.txt"), ".jpg");
    assert_eq!(replaced, std::path::PathBuf::from("notes.jpg"));
    assert_eq!(old_ext.as_deref(), Some(".txt"));
    let (same, no_old) =
        telegram_mcp::state::target_path(std::path::Path::new("photo.JPG"), ".jpg");
    assert_eq!(same, std::path::PathBuf::from("photo.JPG"));
    assert_eq!(no_old, None);

    assert!(
        telegram_mcp::tools::messaging::media::uploads::ensure_extension_allowed(
            "send_voice",
            std::path::Path::new("voice.ogg"),
        )
        .is_ok()
    );
    assert!(
        telegram_mcp::tools::messaging::media::uploads::ensure_extension_allowed(
            "send_voice",
            std::path::Path::new("voice.oga"),
        )
        .is_err()
    );
    assert!(
        telegram_mcp::tools::messaging::media::uploads::ensure_extension_allowed(
            "send_sticker",
            std::path::Path::new("s.webp"),
        )
        .is_ok()
    );
    assert!(
        telegram_mcp::tools::messaging::media::uploads::ensure_extension_allowed(
            "send_sticker",
            std::path::Path::new("s.png"),
        )
        .is_err()
    );
    assert_eq!(
        sanitize_download_filename("report.PDF").unwrap(),
        "report.PDF"
    );
    assert_eq!(sanitize_download_filename("notes.").unwrap(), "notes.bin");
    assert_eq!(sanitize_download_filename("README").unwrap(), "README");

    for bad in [
        "~/secret.txt",
        "~alice/out.txt",
        "dir/*.txt",
        "dir/a?.txt",
        "dir/[a].txt",
        "doc.txt:hidden",
    ] {
        assert!(
            validate_path_syntax(Path::new(bad)).is_err(),
            "must reject {bad}"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let state = State::at(temp.path().join("state"), vec![root.clone()]).unwrap();
    let spaced = root.join("my folder").join("report 2026.PDF");
    let resolved = state.resolve_download_file(&spaced).unwrap();
    assert!(resolved.ends_with(Path::new("my folder").join("report 2026.PDF")));
    let blocked_resolved = state
        .resolve_download_file(&root.join("installer.EXE. "))
        .unwrap();
    assert_eq!(blocked_resolved.file_name().unwrap(), "installer.bin");
    assert!(
        state
            .resolve_download_file(&root.join("sub~1").join("file.txt"))
            .is_err()
    );
    assert!(sanitize_download_path(Path::new("a:stream")).is_err());
}

#[test]
fn always_allowed_folders_extend_effective_roots_without_opening_protected_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    let extra = temp.path().join("approved-dir");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&extra).unwrap();
    let state = State::at(temp.path().join("state"), vec![root]).unwrap();
    let target = extra.join("note.txt");
    assert!(state.resolve_file(&target, FileAccess::Write).is_err());
    state
        .write_json(
            "always-approvals.json",
            &json!({"grants": [], "folders": [extra]}),
        )
        .unwrap();
    assert!(state.resolve_file(&target, FileAccess::Write).is_ok());
    assert!(
        state
            .resolve_file(&state.root().join("leak.txt"), FileAccess::Write)
            .is_err()
    );
}

#[test]
fn tool_timeout_and_refusal_overrides_follow_contract() {
    assert_eq!(
        parse_tool_timeout(None, 55.0),
        Some(Duration::from_secs(55))
    );
    assert_eq!(
        parse_tool_timeout(Some(""), 55.0),
        Some(Duration::from_secs(55))
    );
    assert_eq!(
        parse_tool_timeout(Some("not-a-number"), 55.0),
        Some(Duration::from_secs(55))
    );
    assert_eq!(
        parse_tool_timeout(Some("NaN"), 55.0),
        Some(Duration::from_secs(55))
    );
    assert_eq!(
        parse_tool_timeout(Some("inf"), 55.0),
        Some(Duration::from_secs(55))
    );
    assert_eq!(parse_tool_timeout(Some("0"), 55.0), None);
    assert_eq!(parse_tool_timeout(Some("-3.5"), 55.0), None);
    assert_eq!(
        parse_tool_timeout(Some("12.5"), 55.0),
        Some(Duration::from_secs_f64(12.5))
    );

    let refusals = RefusalOverrides::parse(
        Some(r#"{"FLOOD_WAIT":"Slow down please","bad_lower":"ignored","HTML_KEY":"<b>nope</b>"}"#),
        Some(r#"{"403":"Permission denied by policy","999":"out of range","abc":"bad"}"#),
    );
    assert_eq!(
        refusals.render(Some("FLOOD_WAIT"), None, "default"),
        "Slow down please"
    );
    assert_eq!(
        refusals.render(Some("HTML_KEY"), None, "safe default"),
        "safe default"
    );
    assert_eq!(
        refusals.render(None, Some(403), "default"),
        "Permission denied by policy"
    );
    assert_eq!(
        refusals.render(None, Some(999), "safe default"),
        "safe default"
    );
    assert_eq!(
        refusals.render_error_text("Telegram RPC FLOOD_WAIT_30"),
        "Slow down please"
    );
}

#[test]
fn client_identity_overrides_apply_and_reject_unsafe_values() {
    let valid = ClientIdentity::from_snapshot(&snapshot(&[
        ("TELEGRAM_DEVICE_MODEL", "Workstation X"),
        ("TELEGRAM_SYSTEM_VERSION", "Windows 11"),
        ("TELEGRAM_APP_VERSION", "3.2.0-custom"),
    ]))
    .unwrap();
    let mut params = grammers_mtsender::ConnectionParams::default();
    valid.apply_to(&mut params);
    assert_eq!(params.device_model, "Workstation X");
    assert_eq!(params.system_version, "Windows 11");
    assert_eq!(params.app_version, "3.2.0-custom");

    let err = ClientIdentity::from_snapshot(&snapshot(&[(
        "TELEGRAM_DEVICE_MODEL",
        "secret-value\ninjected",
    )]))
    .unwrap_err();
    assert!(!err.to_string().contains("secret-value"));
}

#[test]
fn proxy_precedence_conflicts_rdns_and_mtproto_auth_are_enforced() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let state = State::at(temp.path().join("state"), vec![root]).unwrap();

    // Account URL wins over global typed settings.
    let cfg = snapshot(&[
        ("TELEGRAM_PROXY_TYPE", "http"),
        ("TELEGRAM_PROXY_HOST", "global.example"),
        ("TELEGRAM_PROXY_PORT", "8080"),
        ("TELEGRAM_PROXY_URL_WORK", "socks5://acct.example:1080"),
    ]);
    let choices = proxy::candidates(&state, &cfg, "work").unwrap();
    let chosen = choices[0].proxy.as_ref().unwrap();
    assert_eq!(chosen.kind, "socks5");
    assert_eq!(chosen.host, "acct.example");
    assert_eq!(chosen.port, 1080);

    // Same-scope conflict between URL and typed fields fails closed.
    let conflict = snapshot(&[
        ("TELEGRAM_PROXY_URL", "http://a.example:80"),
        ("TELEGRAM_PROXY_HOST", "b.example"),
    ]);
    assert!(proxy::candidates(&state, &conflict, "default").is_err());

    // Invalid RDNS or conflicting RDNS=false with socks5h fails closed.
    let bad_rdns = snapshot(&[
        ("TELEGRAM_PROXY_TYPE", "socks5"),
        ("TELEGRAM_PROXY_HOST", "p.example"),
        ("TELEGRAM_PROXY_PORT", "1080"),
        ("TELEGRAM_PROXY_RDNS", "maybe"),
    ]);
    assert!(proxy::candidates(&state, &bad_rdns, "default").is_err());

    let conflict_rdns = snapshot(&[
        ("TELEGRAM_PROXY_TYPE", "socks5h"),
        ("TELEGRAM_PROXY_HOST", "p.example"),
        ("TELEGRAM_PROXY_PORT", "1080"),
        ("TELEGRAM_PROXY_RDNS", "false"),
    ]);
    assert!(proxy::candidates(&state, &conflict_rdns, "default").is_err());

    // MTProto rejects username/password auth.
    let synthetic_secret = "ab".repeat(16);
    let mt_auth = snapshot(&[
        ("TELEGRAM_PROXY_TYPE", "mtproto"),
        ("TELEGRAM_PROXY_HOST", "mt.example"),
        ("TELEGRAM_PROXY_PORT", "443"),
        ("TELEGRAM_PROXY_SECRET", &synthetic_secret),
        ("TELEGRAM_PROXY_USERNAME", "alice"),
    ]);
    assert!(proxy::candidates(&state, &mt_auth, "default").is_err());
}

#[tokio::test]
async fn lock_grace_bounds_monotonic_retry_and_cancellation() {
    assert_eq!(
        parse_lock_grace_seconds(None).unwrap(),
        Duration::from_secs_f64(DEFAULT_LOCK_GRACE_SECONDS)
    );
    assert_eq!(parse_lock_grace_seconds(Some("0")).unwrap(), Duration::ZERO);
    assert_eq!(
        parse_lock_grace_seconds(Some("300")).unwrap(),
        Duration::from_secs(300)
    );
    for invalid in ["-1", "300.1", "NaN", "inf", "abc"] {
        assert!(
            parse_lock_grace_seconds(Some(invalid)).is_err(),
            "must reject {invalid}"
        );
    }

    let attempts = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&attempts);
    let err = acquire_with_grace(Duration::ZERO, None, move || -> anyhow::Result<()> {
        c.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("busy")
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("busy"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let retries = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&retries);
    let got = acquire_with_grace(Duration::from_secs(2), None, move || {
        if r.fetch_add(1, Ordering::SeqCst) < 2 {
            anyhow::bail!("busy")
        }
        Ok(42usize)
    })
    .await
    .unwrap();
    assert_eq!(got, 42);

    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let cancelled: anyhow::Result<()> =
        acquire_with_grace(Duration::from_secs(5), Some(&token), || {
            anyhow::bail!("busy")
        })
        .await;
    assert!(cancelled.unwrap_err().to_string().contains("cancelled"));
}

#[test]
fn pooled_sessions_parse_separators_and_deduplicate_auth_keys() {
    let s1_a = legacy_string_session(2, [149, 154, 167, 50], 443, 0x11);
    let s1_b = legacy_string_session(2, [149, 154, 167, 51], 443, 0x11);
    let s2 = legacy_string_session(4, [149, 154, 167, 91], 443, 0x22);
    let s3 = legacy_string_session(5, [91, 108, 56, 100], 443, 0x33);
    assert_eq!(
        session_auth_key_identity(&s1_a),
        session_auth_key_identity(&s1_b)
    );

    let cfg = snapshot(&[
        ("TELEGRAM_SESSION_STRING_WORK", &s2),
        (
            "TELEGRAM_SESSION_STRINGS",
            &format!("{s1_a}, {s1_b}; {s2}\n{s3}"),
        ),
    ]);
    let pool = pooled_sessions(&cfg);
    assert_eq!(pool.len(), 2);
    assert_eq!(pool[0].value, s1_a);
    assert_eq!(pool[1].value, s3);
}

#[tokio::test]
async fn event_feed_honors_custom_file_pre_touches_and_updates_settle_ms_live() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let custom_feed = root.join("my feed.jsonl");
    let state = Arc::new(
        State::at(temp.path().join("state"), vec![root])
            .unwrap()
            .with_feed_file(Some(custom_feed.clone())),
    );
    assert!(!custom_feed.exists());
    let enabled = state.enable_incoming_feed(500).unwrap();
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["settle_ms"], 500);
    assert!(custom_feed.is_file());
    assert!(
        enabled["watch_command"]
            .as_str()
            .unwrap()
            .contains("my feed.jsonl")
    );

    // Live settle_ms update while running replaces the worker cleanly.
    let updated = state.enable_incoming_feed(150).unwrap();
    assert_eq!(updated["enabled"], true);
    assert_eq!(updated["settle_ms"], 150);

    let disabled = state.disable_incoming_feed().unwrap();
    assert_eq!(disabled["enabled"], false);
}

#[test]
fn live_alias_file_and_canonical_ghost_keys_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let custom_aliases = root.join("live-aliases.json");
    let state = State::at(temp.path().join("state"), vec![root])
        .unwrap()
        .with_live_alias_file(Some(custom_aliases.clone()));

    let saved = state
        .alias_set_with_name(
            "default",
            "boss",
            json!({"_": "inputPeerUser", "user_id": "42", "access_hash": "99"}),
            Some("Alice\u{202E} Smith"),
            false,
        )
        .unwrap();
    assert_eq!(saved["name"], "Alice Smith");
    assert!(custom_aliases.is_file());
    assert!(!state.root().join("aliases-v2.json").exists());
    let loaded = state.aliases("default").unwrap();
    assert_eq!(loaded["boss"]["name"], "Alice Smith");

    // Canonical marked ID + @username both match in ghost.json.
    state
        .set_ghost_mode_for_keys(false, Some("default"), &["42".into(), "alice".into()])
        .unwrap();
    assert_eq!(
        state.ghost_mode(Some("default"), Some(&json!("@alice")))["effective"],
        false
    );
    assert_eq!(
        state.ghost_mode(Some("default"), Some(&json!(42)))["effective"],
        false
    );
}

#[test]
fn structured_content_elicitation_and_preflight_contracts() {
    let tools = catalogue().unwrap();
    let accept_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "accept_secret_chat")
        .expect("accept_secret_chat in catalogue");
    assert!(
        accept_tool.output_schema.is_some(),
        "accept_secret_chat must expose outputSchema"
    );
    let payload = json!({
        "status": "ready",
        "chat_id": 77,
        "wire_chat_id": 77,
        "account": "default",
    });
    let call_res = build_call_tool_result(Some(accept_tool), payload.clone());
    assert_eq!(call_res.structured_content, Some(payload));

    let plain_res = build_call_tool_result(None, json!({"ok": true}));
    assert!(plain_res.structured_content.is_none());

    let user_block = user_audience_text("Visible to user");
    let text = user_block.as_text().unwrap();
    assert_eq!(
        text.annotations.as_ref().and_then(|a| a.audience.as_ref()),
        Some(&vec![Role::User])
    );

    let mut ctx = TrustedRequestContext::local();
    assert!(!ctx.supports_ordinary_elicitation());
    ctx.supports_form_elicitation = true;
    ctx.protocol_version = Some("2025-11-25".into());
    assert!(ctx.supports_ordinary_elicitation());
    ctx.protocol_version = Some("2026-07-28".into());
    assert!(!ctx.supports_ordinary_elicitation());

    let missing = missing_export_choices(&json!({"secret_chat_id": 1}));
    assert!(!missing.is_empty());
    assert!(build_export_choice_schema(&missing).is_ok());
    let mut raw = serde_json::Map::new();
    let mut elicited = serde_json::Map::new();
    elicited.insert("format".into(), json!("json"));
    elicited.insert("ignored_extra".into(), json!("nope"));
    merge_elicited_choices(&mut raw, &missing, &elicited);
    assert_eq!(raw.get("format"), Some(&json!("json")));
    assert!(!raw.contains_key("ignored_extra"));

    // Preflight validates send_typing before Ghost Mode / peer I/O.
    assert!(preflight_validate_tool("send_typing", &json!({"seconds": 0.1})).is_err());
    assert!(preflight_validate_tool("send_typing", &json!({"seconds": 46.0})).is_err());
    assert!(
        preflight_validate_tool("send_typing", &json!({"seconds": 5.0, "topic_id": 0})).is_err()
    );
    assert!(
        preflight_validate_tool("send_typing", &json!({"seconds": 5.0, "topic_id": 12})).is_ok()
    );
    assert!(preflight_validate_tool("accept_secret_chat", &json!({"chat_id": 0})).is_err());
    assert!(preflight_validate_tool("accept_secret_chat", &json!({"chat_id": 42})).is_ok());

    // Permalink domain recognition.
    assert_eq!(
        parse_domain(Some("https://tg.example/")).unwrap(),
        "tg.example"
    );
    assert_eq!(
        strip_username_prefix("https://tg.example/alice", Some("tg.example")),
        "alice"
    );
    assert!(identifier_like("https://t.me/alice"));

    // Roots status distinguishes unavailable vs empty vs advertised client roots.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    let extra = temp.path().join("extra");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&extra).unwrap();
    let state = State::at(temp.path().join("state"), vec![root]).unwrap();
    state
        .write_json(
            "always-approvals.json",
            &json!({"grants": [], "folders": [extra]}),
        )
        .unwrap();
    let unavail = roots_status_payload(&state, None);
    assert_eq!(unavail["client_roots_status"], "unavailable");
    assert!(unavail["client_roots"].is_null());
    assert_eq!(unavail["free_folders"].as_array().unwrap().len(), 1);
    assert_eq!(
        unavail["always_allowed_folders"].as_array().unwrap().len(),
        1
    );
    let empty = roots_status_payload(&state, Some(vec![]));
    assert_eq!(empty["client_roots_status"], "empty");
    let advertised = roots_status_payload(&state, Some(vec![PathBuf::from("C:/workspace")]));
    assert_eq!(advertised["client_roots_status"], "advertised");
}
