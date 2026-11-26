use super::common::*;
use serde_json::json;

#[tokio::test]
async fn cancelled_member_removal_holds_account_serialization_until_cleanup_finishes() {
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use telegram_mcp::{backend::Telegram, tools::identity};
    struct Owned {
        mock: Arc<Mock>,
        token: Option<Arc<dyn Send + Sync>>,
        calls: Arc<AtomicUsize>,
        cleanup: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl Telegram for Owned {
        async fn resolve_peer(&self, p: &Value) -> anyhow::Result<Value> {
            self.mock.resolve_peer(p).await
        }
        async fn invoke(&self, name: &str, args: Value) -> anyhow::Result<Value> {
            if name == "channels.editBanned" && self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                self.cleanup.notify_one();
                self.release.notified().await;
            }
            self.mock.invoke(name, args).await
        }
        fn operation_guard(&self) -> Option<Arc<dyn Send + Sync>> {
            self.token.clone()
        }
        fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
            Some(Arc::new(Self {
                mock: self.mock.clone(),
                token: None,
                calls: self.calls.clone(),
                cleanup: self.cleanup.clone(),
                release: self.release.clone(),
            }))
        }
    }
    let serial = Arc::new(tokio::sync::Mutex::new(()));
    let cleanup = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let api = Owned {
        mock: Arc::new(Mock::new()),
        token: Some(Arc::new(serial.clone().lock_owned().await)),
        calls: Arc::new(AtomicUsize::new(0)),
        cleanup: cleanup.clone(),
        release: release.clone(),
    };
    let caller = tokio::spawn(async move {
        identity::call(&api, "kick_user", &json!({"chat_id":7,"user_id":8}))
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), cleanup.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(
        serial.try_lock().is_err(),
        "A later ban could run before the detached cleanup"
    );
    release.notify_one();
    let _released = tokio::time::timeout(std::time::Duration::from_secs(2), serial.lock())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_lost_invitation_response_is_unknown_and_does_not_invite_a_retry() {
    let m = Mock::new();
    m.fail_with(
        "messages.addChatUser",
        "Telegram transport failed; operation outcome may be unknown and was not retried",
    );
    let result = run(&m, "invite_to_group", json!({"group_id":9,"user_ids":[8]}))
        .await
        .unwrap();
    assert!(result["results"][0]["success"].is_null());
    assert!(
        result["results"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("membership")
    );
    assert_eq!(m.sent("messages.addChatUser").len(), 1);
}

#[tokio::test]
async fn replacing_a_chat_photo_collects_all_pages_and_removes_every_prior_photo_in_batches() {
    for group in [7, 9] {
        let m = Mock::new();
        let first=(102..=201).rev().map(|id|json!({"_":"messageService","id":id,"action":{"_":"messageActionChatEditPhoto"}})).collect::<Vec<_>>();
        m.reply("messages.search", json!({"messages":first}));
        m.reply("messages.search",json!({"messages":[{"_":"messageService","id":101,"action":{"_":"messageActionChatEditPhoto"}}]}));
        let result = run(
            &m,
            "replace_chat_photo",
            json!({"chat_id":group,"file_path":"new.png"}),
        )
        .await
        .unwrap();
        assert_eq!(m.sent("messages.search").len(), 2);
        assert_eq!(m.sent("messages.search")[1]["offset_id"], 102);
        let method = if group == 7 {
            "channels.deleteMessages"
        } else {
            "messages.deleteMessages"
        };
        let calls = m.sent(method);
        assert_eq!(calls.len(), 2);
        let removed = calls
            .iter()
            .flat_map(|call| call["id"].as_array().unwrap().iter())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(removed.len(), 101);
        assert!(removed.contains(&json!(101)));
        assert_eq!(result["history_removed"], true);
    }
}

#[tokio::test]
async fn deleting_an_already_left_basic_group_finishes_history_cleanup() {
    let m = Mock::new();
    m.fail_with(
        "messages.deleteChatUser",
        "Telegram RPC 400 USER_NOT_PARTICIPANT",
    );
    m.reply("messages.deleteHistory", json!({"offset":2}));
    m.reply("messages.deleteHistory", json!({"offset":0}));
    let result = run(&m, "delete_chat", json!({"chat":9,"both_sides":false}))
        .await
        .unwrap();
    assert_eq!(result["complete"], true);
    assert_eq!(m.sent("messages.deleteHistory").len(), 2);
}

#[tokio::test]
async fn unknown_or_other_leave_failures_do_not_start_history_deletion() {
    for reason in [
        "Telegram transport failed; USER_NOT_PARTICIPANT outcome unknown",
        "Telegram RPC 400 CHAT_ADMIN_REQUIRED",
    ] {
        let m = Mock::new();
        m.fail_with("messages.deleteChatUser", reason);
        assert!(
            run(&m, "delete_chat", json!({"chat":9,"both_sides":false}))
                .await
                .is_err()
        );
        assert!(m.sent("messages.deleteHistory").is_empty());
    }
}

#[tokio::test]
async fn basic_group_demotion_uses_the_basic_group_admin_model() {
    for accepted in [true, false] {
        let m = Mock::new();
        m.reply("messages.editChatAdmin", json!(accepted));
        let result = run(&m, "demote_admin", json!({"group_id":9,"user_id":8}))
            .await
            .unwrap();
        assert_eq!(result["accepted"], accepted);
        assert_eq!(result["admin_level"], "basic_group");
        assert_eq!(m.sent("messages.editChatAdmin")[0]["is_admin"], false);
        assert!(m.sent("channels.editAdmin").is_empty());
        assert!(m.sent("channels.getParticipant").is_empty());
    }
}

#[tokio::test]
async fn missing_both_sides_refuses_history_deletion_before_network() {
    let m = Mock::new();
    assert!(
        run(&m, "clear_chat_history", json!({"chat":8}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn both_sides_is_not_silently_ignored_for_group() {
    let m = Mock::new();
    assert!(
        run(&m, "delete_chat", json!({"chat":7,"both_sides":true}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn clear_private_history_repeats_batches_until_offset_zero() {
    let m = Mock::new();
    m.reply("messages.deleteHistory", json!({"offset":5}));
    m.reply("messages.deleteHistory", json!({"offset":0}));
    let r = run(
        &m,
        "clear_chat_history",
        json!({"chat":8,"both_sides":true}),
    )
    .await
    .unwrap();
    assert_eq!(r["complete"], true);
    assert_eq!(r["passes"], 2);
    let calls = m.sent("messages.deleteHistory");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["just_clear"], true);
    assert_eq!(calls[0]["revoke"], true);
}
#[tokio::test]
async fn endless_delete_offsets_are_bounded_and_report_partial() {
    let m = Mock::new();
    for _ in 0..50 {
        m.reply("messages.deleteHistory", json!({"offset":1}));
    }
    let r = run(&m, "delete_chat", json!({"chat":8,"both_sides":false}))
        .await
        .unwrap();
    assert_eq!(r["complete"], false);
    assert_eq!(m.sent("messages.deleteHistory").len(), 50);
}
#[tokio::test]
async fn deleting_channel_from_list_leaves_without_destroying_channel() {
    let m = Mock::new();
    run(&m, "delete_chat", json!({"chat":7,"both_sides":false}))
        .await
        .unwrap();
    assert_eq!(m.methods(), ["channels.leaveChannel"]);
}
#[tokio::test]
async fn clearing_broadcast_history_is_refused() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","broadcast":true}));
    assert!(
        run(
            &m,
            "clear_chat_history",
            json!({"chat":7,"both_sides":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.deleteHistory").is_empty());
}
#[tokio::test]
async fn kick_lifts_ban_after_removing_member() {
    let m = Mock::new();
    run(&m, "kick_user", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    let calls = m.sent("channels.editBanned");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["banned_rights"]["view_messages"], true);
    assert!(calls[1]["banned_rights"].get("view_messages").is_none());
}
#[tokio::test]
async fn failed_kick_unban_reports_member_remains_banned() {
    let m = Mock::new();
    m.reply("channels.editBanned", true.into());
    m.fail("channels.editBanned");
    let r = run(&m, "kick_user", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(r["kicked"], true);
    assert_eq!(r["ban_lifted"], false);
}
#[tokio::test]
async fn basic_group_kick_uses_delete_chat_user() {
    let m = Mock::new();
    run(&m, "kick_user", json!({"chat_id":9,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(m.methods(), ["messages.deleteChatUser"]);
    assert_eq!(m.sent("messages.deleteChatUser")[0]["chat_id"], "9");
}
#[tokio::test]
async fn sound_toggle_preserves_mute_and_story_sound() {
    let m = Mock::new();
    m.reply("account.getNotifySettings",json!({"mute_until":2000000000,"silent":false,"show_previews":true,"other_sound":{"_":"notificationSoundDefault"},"stories_muted":true,"stories_other_sound":{"_":"notificationSoundNone"}}));
    run(
        &m,
        "set_chat_sound_disabled",
        json!({"chat":7,"disabled":true}),
    )
    .await
    .unwrap();
    let s = &m.sent("account.updateNotifySettings")[0]["settings"];
    assert_eq!(s["mute_until"], 2000000000_i64);
    assert_eq!(s["silent"], false);
    assert_eq!(s["sound"]["_"], "notificationSoundNone");
    assert_eq!(s["stories_sound"]["_"], "notificationSoundNone");
}

#[tokio::test]
async fn basic_group_invitation_reports_missing_invitees_as_failures_per_user() {
    let m = Mock::new();
    m.set_peer(
        "10",
        json!({"_":"inputPeerUser","user_id":"10","access_hash":"100"}),
    );
    m.reply("messages.addChatUser",json!({"_":"messages.invitedUsers","missing_invitees":[{"_":"missingInvitee","user_id":"8","premium_would_allow_invite":true}]}));
    m.reply("messages.addChatUser",json!({"_":"messages.invitedUsers","updates":{"updates":[{"message":{"peer_id":{"_":"peerChat","chat_id":"9"},"action":{"_":"messageActionChatAddUser","users":["10"]}}}]},"missing_invitees":[{"_":"missingInvitee","user_id":"8"}]}));
    let result = run(
        &m,
        "invite_to_group",
        json!({"group_id":9,"user_ids":[8,10]}),
    )
    .await
    .unwrap();
    assert_eq!(result["results"][0]["success"], false);
    assert_eq!(
        result["results"][0]["missing_invitee"]["premium_would_allow_invite"],
        true
    );
    assert!(
        result["results"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("privacy")
    );
    assert_eq!(result["results"][1]["success"], true);
}

#[tokio::test]
async fn ambiguous_removal_transport_failure_still_clears_ban_and_never_claims_known_success() {
    for name in ["kick_user", "remove_user"] {
        let m = Mock::new();
        m.fail_with(
            "channels.editBanned",
            "Telegram transport failed; operation outcome may be unknown and was not retried",
        );
        let result = run(&m, name, json!({"chat_id":7,"user_id":8}))
            .await
            .unwrap();
        assert!(result["kicked"].is_null());
        assert!(result["ban_lifted"].is_null());
        assert_eq!(result["cleanup_accepted"], true);
        assert_eq!(m.sent("channels.editBanned").len(), 2);
        assert!(
            m.sent("channels.editBanned")[1]["banned_rights"]
                .get("view_messages")
                .is_none()
        );
    }
}

#[tokio::test]
async fn known_rpc_refusal_does_not_issue_an_unrelated_unban() {
    let m = Mock::new();
    m.fail_with(
        "channels.editBanned",
        "Telegram RPC 400 CHAT_ADMIN_REQUIRED",
    );
    assert!(
        run(&m, "kick_user", json!({"chat_id":7,"user_id":8}))
            .await
            .is_err()
    );
    assert_eq!(m.sent("channels.editBanned").len(), 1);
}

#[tokio::test]
async fn a_lost_cleanup_response_never_claims_the_member_is_still_banned() {
    let m = Mock::new();
    m.reply("channels.editBanned", json!(true));
    m.fail_with(
        "channels.editBanned",
        "Telegram transport failed; operation outcome may be unknown and was not retried",
    );
    let result = run(&m, "kick_user", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(result["kicked"], true);
    assert!(result["ban_lifted"].is_null());
    assert_eq!(m.sent("channels.editBanned").len(), 2);
}
#[tokio::test]
async fn mute_zero_duration_means_forever_without_erasing_tone() {
    let m = Mock::new();
    m.reply(
        "account.getNotifySettings",
        json!({"other_sound":{"_":"notificationSoundRingtone","id":"8"}}),
    );
    run(&m, "mute_chat", json!({"chat_id":7})).await.unwrap();
    let s = &m.sent("account.updateNotifySettings")[0]["settings"];
    assert_eq!(s["mute_until"], i32::MAX);
    assert_eq!(s["sound"]["id"], "8");
}
#[tokio::test]
async fn negative_mute_duration_refuses_before_network() {
    let m = Mock::new();
    assert!(
        run(&m, "mute_chat", json!({"chat_id":7,"minutes":-1}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn choosing_tone_requires_exactly_one_source() {
    let m = Mock::new();
    assert!(run(&m, "set_chat_tone", json!({"chat":7})).await.is_err());
    assert!(
        run(
            &m,
            "set_chat_tone",
            json!({"chat":7,"tone":"none","file_path":"foo.ogg"})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn notification_tone_refuses_invalid_file_type_before_upload() {
    let m = Mock::new();
    assert!(
        run(&m, "set_chat_tone", json!({"chat":7,"file_path":"foo.wav"}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}

#[tokio::test]
async fn channel_invite_reports_observed_members_and_privacy_refusals() {
    let m = Mock::new();
    m.set_peer(
        "10",
        json!({"_":"inputPeerUser","user_id":"10","access_hash":"100"}),
    );
    m.reply("channels.inviteToChannel",json!({"_":"messages.invitedUsers","missing_invitees":[{"user_id":"8","premium_required_for_pm":true}],"updates":{"updates":[{"message":{"peer_id":{"channel_id":"7"},"action":{"_":"messageActionChatAddUser","users":["10","8","23"]}}}]}}));
    let r = run(
        &m,
        "invite_to_group",
        json!({"group_id":7,"user_ids":[8,10]}),
    )
    .await
    .unwrap();
    assert_eq!(r["confirmed_added"], json!(["10"]));
    assert_eq!(r["missing_invitees"][0]["user_id"], "8");
    assert!(
        r["missing_invitees"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("Premium")
    );
}
