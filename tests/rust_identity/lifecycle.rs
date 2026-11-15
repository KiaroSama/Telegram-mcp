use super::common::*;
use serde_json::json;

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
    m.reply("messages.addChatUser",json!({"_":"messages.invitedUsers","missing_invitees":[{"_":"missingInvitee","user_id":"8"}]}));
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
