mod local_dispatch_support;
use local_dispatch_support::{Api, call, isolated};
use serde_json::json;
use telegram_mcp::state::{IncomingEvent, State};

#[tokio::test]
async fn account_alias_settings_and_grant_status_are_durable_through_local_dispatch() {
    if !isolated("account_alias_settings_and_grant_status_are_durable_through_local_dispatch") {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    assert_eq!(
        call(&api, "list_accounts", json!({})).await.unwrap()["accounts"][0]["label"],
        "work"
    );
    let health = call(
        &api,
        "check_account_session",
        json!({"config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":"digest"}),
    )
    .await
    .unwrap();
    assert_eq!(health["status"], "unknown");
    assert_eq!(api.calls()[1].1["config_digest"], "digest");
    let roots = call(&api, "get_file_roots_status", json!({}))
        .await
        .unwrap();
    assert_eq!(roots["file_tools_enabled"], true);
    assert_eq!(roots["roots"].as_array().unwrap().len(), 1);
    assert!(
        call(
            &api,
            "set_contact_alias",
            json!({"alias":"close friend","chat_id":42})
        )
        .await
        .is_err()
    );
    for alias in ["close friend", "old friend"] {
        call(
            &api,
            "set_contact_alias",
            json!({"account":"work","alias":alias,"chat_id":42}),
        )
        .await
        .unwrap();
    }
    call(
        &api,
        "set_contact_alias",
        json!({"account":"personal","alias":"close friend","chat_id":99}),
    )
    .await
    .unwrap();
    let aliases = call(&api, "list_contact_aliases", json!({"account":"work"}))
        .await
        .unwrap();
    assert_eq!(aliases.as_array().unwrap().len(), 1);
    assert_eq!(aliases[0]["aliases"].as_array().unwrap().len(), 2);
    assert_eq!(aliases[0]["id"], "42");
    assert!(
        call(
            &api,
            "set_contact_alias",
            json!({"account":"work","alias":"close friend","chat_id":99})
        )
        .await
        .is_err()
    );
    assert_eq!(
        call(
            &api,
            "delete_contact_alias",
            json!({"account":"work","alias":"old friend"})
        )
        .await
        .unwrap()["deleted"],
        true
    );
    assert!(
        state
            .alias_lookup("personal", "close friend")
            .unwrap()
            .is_some()
    );
    call(
        &api,
        "set_ghost_mode",
        json!({"account":"work","chat_id":42,"enabled":false}),
    )
    .await
    .unwrap();
    assert_eq!(
        call(
            &api,
            "get_ghost_mode",
            json!({"account":"work","chat_id":42})
        )
        .await
        .unwrap()["effective"],
        false
    );
    assert_eq!(
        call(
            &api,
            "get_ghost_mode",
            json!({"account":"personal","chat_id":42})
        )
        .await
        .unwrap()["effective"],
        true
    );
    let folder = state.file_roots()[0].canonicalize().unwrap();
    state.write_json("always-approvals.json",&json!({"grants":[["work","delete_message","42"],["personal","delete_message","42"]],"folders":[folder]})).unwrap();
    let status = call(&api, "safeguard_status", json!({})).await.unwrap();
    assert_eq!(status["always_approved"].as_array().unwrap().len(), 2);
    assert_eq!(status["bypass"], false);
    assert_eq!(
        call(&api, "revoke_always_approval", json!({"folder":folder}))
            .await
            .unwrap()["revoked"],
        true
    );
    assert_eq!(
        call(&api, "revoke_always_approval", json!({"folder":folder}))
            .await
            .unwrap()["revoked"],
        false
    );
    assert_eq!(
        call(
            &api,
            "revoke_always_approval",
            json!({"account":"work","tool":"delete_message","chat_id":42})
        )
        .await
        .unwrap()["revoked"],
        true
    );
    assert!(state.approval_is_granted("personal", "delete_message", &json!(42)));
    assert!(
        telegram_mcp::tools::local::call(&api, "not_a_local_tool", &json!({}))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn incoming_dispatch_resolves_targets_and_consumes_only_settled_bursts() {
    if !isolated("incoming_dispatch_resolves_targets_and_consumes_only_settled_bursts") {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    state
        .record_incoming(IncomingEvent {
            account: "work".into(),
            chat_id: "42".into(),
            message_id: "7".into(),
            name: "Synthetic".into(),
            username: None,
            bot: false,
        })
        .unwrap();
    let first = call(
        &api,
        "wait_for_new_message",
        json!({"account":"work","chat_id":42,"timeout":0.02}),
    )
    .await
    .unwrap();
    assert_eq!(first["event"], true);
    let settled = call(
        &api,
        "wait_for_settled_message",
        json!({"account":"work","chat_id":42,"max_wait_ms":50,"settle_ms":1}),
    )
    .await
    .unwrap();
    assert_eq!(settled["last_message_id"], "7");
    assert_eq!(
        call(
            &api,
            "wait_for_new_message",
            json!({"account":"work","chat_id":42,"timeout":0.001})
        )
        .await
        .unwrap()["event"],
        false
    );
    for args in [
        json!({"timeout":0}),
        json!({"timeout":51}),
        json!({"timeout":"50"}),
    ] {
        assert!(call(&api, "wait_for_new_message", args).await.is_err());
    }
    assert!(
        call(
            &api,
            "wait_for_new_message",
            json!({"chat_id":"me","timeout":0.001})
        )
        .await
        .is_err()
    );
    for target in ["group", "channel"] {
        assert_eq!(
            call(
                &api,
                "wait_for_new_message",
                json!({"chat_id":target,"timeout":0.001})
            )
            .await
            .unwrap()["event"],
            false
        );
    }
    assert!(
        call(&api, "enable_incoming_feed", json!({"settle_ms":0}))
            .await
            .is_err()
    );
    assert_eq!(
        call(&api, "enable_incoming_feed", json!({"settle_ms":1}))
            .await
            .unwrap()["enabled"],
        true
    );
    assert_eq!(
        call(&api, "incoming_feed_status", json!({})).await.unwrap()["enabled"],
        true
    );
    assert_eq!(
        call(&api, "disable_incoming_feed", json!({}))
            .await
            .unwrap()["enabled"],
        false
    );
    assert!(api.calls().is_empty());
}

#[tokio::test]
async fn bot_command_previews_use_scope_qualification_filtering_and_real_total() {
    let api = Api::default();
    let info = json!({"user_id":"42","commands":[{"command":"Start","description":"Start"},{"command":"stats","description":"Stats"},{"command":"","description":"Empty"}]});
    api.reply(
        "users.getFullUser",
        json!({"full_user":{"bot_info":info},"users":[{"id":"42","username":"helper_bot"}]}),
    );
    let private = call(
        &api,
        "list_chat_commands",
        json!({"chat_id":42,"prefix":"/ST","limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(private["commands"][0]["send_as_text"], "/Start");
    assert_eq!(private["matched"], 2);
    assert_eq!(private["has_more"], true);
    assert_eq!(private["bots"][0]["command_count"], 3);
    api.reply("messages.getFullChat",json!({"full_chat":{"bot_info":[info,{"user_id":"43","commands":[{"command":"info","description":"Info"}]}]},"users":[{"id":"42","username":"helper_bot"}]}));
    let group = call(&api, "list_chat_commands", json!({"chat_id":"group"}))
        .await
        .unwrap();
    assert_eq!(group["chat_scope"], "group");
    assert_eq!(group["bot_count"], 2);
    assert_eq!(group["commands"][0]["send_as_text"], "/Start@helper_bot");
    assert_eq!(group["commands"][2]["send_as_text"], "/info");
    api.reply(
        "channels.getFullChannel",
        json!({"full_chat":{"bot_info":[info]},"users":[]}),
    );
    let no_match = call(
        &api,
        "list_chat_commands",
        json!({"chat_id":"channel","prefix":"unpublished"}),
    )
    .await
    .unwrap();
    assert!(no_match["note"].as_str().unwrap().contains("no matching"));
    api.reply("users.getFullUser", json!({"full_user":{},"users":[]}));
    let absent = call(&api, "list_chat_commands", json!({"chat_id":"me"}))
        .await
        .unwrap();
    assert!(absent["note"].as_str().unwrap().contains("No bot"));
    assert!(
        call(&api, "list_chat_commands", json!({"chat_id":"unsupported"}))
            .await
            .is_err()
    );
    assert!(
        call(&api, "list_chat_commands", json!({"chat_id":42,"limit":0}))
            .await
            .is_err()
    );
    assert_eq!(api.calls()[0].0, "users.getFullUser");
    assert_eq!(api.calls()[1].0, "messages.getFullChat");
    assert_eq!(api.calls()[2].0, "channels.getFullChannel");
    assert_eq!(api.calls()[3].1["id"]["_"], "inputUserSelf");
}

#[tokio::test]
async fn incoming_feed_rotates_private_metadata_and_retains_pending_bursts_on_corruption() {
    use std::time::Duration;
    if !local_dispatch_support::isolated_with(
        "incoming_feed_rotates_private_metadata_and_retains_pending_bursts_on_corruption",
        &[
            ("TELEGRAM_EVENT_FEED_MAX_BYTES", "800"),
            ("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "10"),
        ],
    ) {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    let path = state.root().join("incoming_feed.jsonl");
    let rotated = state.root().join("incoming_feed.jsonl.1");
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({"ts":chrono::Utc::now().timestamp(),"padding":"x".repeat(600)})
        ),
    )
    .unwrap();
    std::fs::write(&rotated, format!("{}\n", json!({"ts":0,"expired":true}))).unwrap();
    state
        .record_incoming(IncomingEvent {
            account: "work".into(),
            chat_id: "42".into(),
            message_id: "7".into(),
            name: "Synthetic".into(),
            username: None,
            bot: false,
        })
        .unwrap();
    call(&api, "enable_incoming_feed", json!({"settle_ms":1}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if std::fs::read_to_string(&path).is_ok_and(|text| text.contains("last_message_id")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    call(&api, "disable_incoming_feed", json!({}))
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&rotated)
            .unwrap()
            .contains("padding")
    );
    assert!(std::fs::metadata(&path).unwrap().len() <= 800);
    let summary: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
    assert_eq!(summary["last_message_id"], "7");
    assert!(summary["ts"].is_number());
    assert!(summary.get("text").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::write(&path, "invalid private metadata\n").unwrap();
    state
        .record_incoming(IncomingEvent {
            account: "work".into(),
            chat_id: "42".into(),
            message_id: "8".into(),
            name: "Synthetic".into(),
            username: None,
            bot: false,
        })
        .unwrap();
    call(&api, "enable_incoming_feed", json!({"settle_ms":1}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state.incoming_feed_status().unwrap()["error"].is_string() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    call(&api, "disable_incoming_feed", json!({}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "invalid private metadata\n"
    );
    assert_eq!(
        call(
            &api,
            "wait_for_new_message",
            json!({"account":"work","chat_id":42,"timeout":0.001})
        )
        .await
        .unwrap()["pending_chats"][0]["last_message_id"],
        "8"
    );
}
