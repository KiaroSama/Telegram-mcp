use super::common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn resolve_phone_normalizes_international_format_without_adding_a_contact() {
    let m = Mock::new();
    m.reply(
        "contacts.resolvePhone",
        json!({"peer":{"_":"peerUser","user_id":"8"},"users":[{"id":"8","first_name":"Ada"}]}),
    );
    let result = run(
        &m,
        "resolve_phone",
        json!({"phone":" +44 (20) 7946-0958. "}),
    )
    .await
    .unwrap();
    assert_eq!(m.methods(), ["contacts.resolvePhone"]);
    assert_eq!(m.sent("contacts.resolvePhone")[0]["phone"], "442079460958");
    assert_eq!(result["users"][0]["first_name"], "Ada");
    assert_eq!(result["untrusted_content"], true);
}

#[tokio::test]
async fn resolve_phone_rejects_local_malformed_unicode_and_overlong_input_before_network() {
    for phone in [
        "89990001122",
        "+012345678",
        "+123456",
        "+1234567890123456",
        "+１２３４５６７",
        "+1/234567",
        "+1\t234567",
        "",
        "+12+34567",
    ] {
        let m = Mock::new();
        assert!(
            run(&m, "resolve_phone", json!({"phone":phone}))
                .await
                .is_err(),
            "accepted {phone:?}"
        );
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn resolve_phone_preserves_private_lookup_failure_without_import_fallback() {
    let m = Mock::new();
    m.fail("contacts.resolvePhone");
    assert!(
        run(&m, "resolve_phone", json!({"phone":"+1234567"}))
            .await
            .is_err()
    );
    assert_eq!(m.methods(), ["contacts.resolvePhone"]);
}

fn topic(m: &Mock) {
    m.reply(
        "messages.getForumTopicsByID",
        json!({"topics":[{"_":"forumTopic","id":4,"top_message":90}]}),
    );
}

#[tokio::test]
async fn topic_read_uses_exact_topic_lookup_and_its_top_message() {
    let m = Mock::new();
    topic(&m);
    let result = run(&m, "mark_topic_as_read", json!({"chat_id":7,"topic_id":4}))
        .await
        .unwrap();
    assert_eq!(result["max_id"], 90);
    assert_eq!(
        m.sent("messages.getForumTopicsByID")[0]["topics"],
        json!([4])
    );
    let sent = &m.sent("messages.readDiscussion")[0];
    assert_eq!(sent["msg_id"], 4);
    assert_eq!(sent["read_max_id"], 90);
    assert!(m.sent("messages.readHistory").is_empty());
    assert!(m.sent("channels.readHistory").is_empty());
}

#[tokio::test]
async fn topic_read_max_id_bounds_the_receipt_and_cannot_exceed_the_current_top() {
    for (maximum, expected) in [(55, 55), (999, 90)] {
        let m = Mock::new();
        topic(&m);
        run(
            &m,
            "mark_topic_as_read",
            json!({"chat_id":7,"topic_id":4,"max_id":maximum}),
        )
        .await
        .unwrap();
        assert_eq!(
            m.sent("messages.readDiscussion")[0]["read_max_id"],
            expected
        );
    }
}

#[tokio::test]
async fn topic_read_structural_bounds_fail_before_network() {
    for args in [
        json!({"chat_id":7,"topic_id":0}),
        json!({"chat_id":7,"topic_id":2147483648i64}),
        json!({"chat_id":7,"topic_id":4,"max_id":-1}),
        json!({"chat_id":7,"topic_id":4,"max_id":2147483648i64}),
    ] {
        let m = Mock::new();
        assert!(run(&m, "mark_topic_as_read", args).await.is_err());
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn topic_read_does_not_emit_receipts_for_missing_topics_or_nonforums() {
    let m = Mock::new();
    m.reply("messages.getForumTopicsByID", json!({"topics":[]}));
    assert!(
        run(&m, "mark_topic_as_read", json!({"chat_id":7,"topic_id":4}))
            .await
            .is_err()
    );
    assert!(m.sent("messages.readDiscussion").is_empty());
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":false}));
    assert!(
        run(&m, "mark_topic_as_read", json!({"chat_id":7,"topic_id":4}))
            .await
            .is_err()
    );
    assert!(m.sent("messages.getForumTopicsByID").is_empty());
}

#[tokio::test]
async fn upgrade_alias_reuses_native_migration_with_positive_basic_chat_id() {
    let m = Mock::new();
    run(&m, "upgrade_group_to_supergroup", json!({"chat_id":9}))
        .await
        .unwrap();
    assert_eq!(m.sent("messages.migrateChat")[0]["chat_id"], "9");
    assert!(m.sent("channels.toggleForum").is_empty());
}

#[tokio::test]
async fn upgrade_alias_detects_existing_supergroups_and_previously_migrated_groups() {
    let m = Mock::new();
    let result = run(&m, "upgrade_group_to_supergroup", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(result["already_supergroup"], true);
    assert!(m.sent("messages.migrateChat").is_empty());
    let m = Mock::new();
    m.reply("messages.getFullChat",json!({"chats":[{"_":"chat","id":"9","migrated_to":{"_":"inputChannel","channel_id":"77","access_hash":"70"}}]}));
    let result = run(&m, "upgrade_group_to_supergroup", json!({"chat_id":9}))
        .await
        .unwrap();
    assert_eq!(result["channel_id"], "77");
    assert!(m.sent("messages.migrateChat").is_empty());
}

#[tokio::test]
async fn upgrade_alias_refuses_broadcast_channels() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","broadcast":true}));
    assert!(
        run(&m, "upgrade_group_to_supergroup", json!({"chat_id":7}))
            .await
            .is_err()
    );
    assert!(m.sent("messages.migrateChat").is_empty());
}

#[tokio::test]
async fn generic_promotion_supports_basic_groups_without_inventing_granular_rights() {
    for accepted in [true, false] {
        let m = Mock::new();
        m.reply("messages.editChatAdmin", json!(accepted));
        let result = run(
            &m,
            "promote_admin",
            json!({"group_id":9,"user_id":8,"rights":{"ban_users":false}}),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], accepted);
        assert_eq!(result["granular_rights_applied"], false);
        assert_eq!(m.sent("messages.editChatAdmin")[0]["is_admin"], true);
        assert!(m.sent("channels.editAdmin").is_empty());
    }
}

#[tokio::test]
async fn generic_promotion_preserves_upstream_defaults_and_named_overrides() {
    let m = Mock::new();
    run(
        &m,
        "promote_admin",
        json!({"group_id":7,"user_id":8,"rights":{"delete_messages":false}}),
    )
    .await
    .unwrap();
    let sent = &m.sent("channels.editAdmin")[0];
    let rights = &sent["admin_rights"];
    assert_eq!(rights["delete_messages"], false);
    assert_eq!(rights["ban_users"], true);
    assert_eq!(rights["add_admins"], false);
    assert_eq!(rights["anonymous"], false);
    assert_eq!(rights["post_stories"], false);
    assert_eq!(sent["rank"], "Admin");
}

#[tokio::test]
async fn generic_promotion_rejects_unknown_or_nonboolean_rights_before_network() {
    for rights in [
        json!({"surprise":true}),
        json!({"delete_messages":"yes"}),
        json!([true]),
    ] {
        let m = Mock::new();
        assert!(
            run(
                &m,
                "promote_admin",
                json!({"group_id":7,"user_id":8,"rights":rights})
            )
            .await
            .is_err()
        );
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn generic_admin_replace_can_demote_and_keeps_unrequested_bits_off() {
    let m = Mock::new();
    run(&m, "edit_admin_rights", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(
        m.sent("channels.editAdmin")[0]["admin_rights"],
        json!({"_":"chatAdminRights"})
    );
    let m = Mock::new();
    run(
        &m,
        "edit_admin_rights",
        json!({"chat_id":7,"user_id":8,"delete_messages":true,"other":true}),
    )
    .await
    .unwrap();
    let rights = &m.sent("channels.editAdmin")[0]["admin_rights"];
    assert_eq!(rights["delete_messages"], true);
    assert_eq!(rights["other"], true);
    assert_eq!(rights["add_admins"], false);
}

#[tokio::test]
async fn generic_admin_patch_retains_unadvertised_rights_and_title() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantAdmin","admin_rights":{"_":"chatAdminRights","delete_messages":true,"manage_welcome_messages":true},"rank":"Keeper"}}));
    run(
        &m,
        "edit_admin_rights",
        json!({"chat_id":7,"user_id":8,"mode":"patch","ban_users":true}),
    )
    .await
    .unwrap();
    let sent = &m.sent("channels.editAdmin")[0];
    assert_eq!(sent["rank"], "Keeper");
    assert_eq!(sent["admin_rights"]["manage_welcome_messages"], true);
    assert_eq!(sent["admin_rights"]["ban_users"], true);
}

#[tokio::test]
async fn generic_admin_refuses_basic_groups_and_nonboolean_demotions() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "edit_admin_rights",
            json!({"chat_id":9,"user_id":8,"delete_messages":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
    let m = Mock::new();
    assert!(
        run(
            &m,
            "edit_admin_rights",
            json!({"chat_id":7,"user_id":8,"delete_messages":"false"})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}

#[tokio::test]
async fn remove_user_clears_the_temporary_ban_after_membership_validation() {
    let m = Mock::new();
    let result = run(&m, "remove_user", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    let sent = m.sent("channels.editBanned");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["banned_rights"]["view_messages"], true);
    assert!(sent[1]["banned_rights"].get("view_messages").is_none());
    assert_eq!(result["ban_lifted"], true);
    assert_eq!(m.methods()[1], "channels.getParticipant");
}

#[tokio::test]
async fn remove_user_preserves_existing_bans_and_refuses_absent_members() {
    for participant in [
        json!({"_":"channelParticipantLeft"}),
        json!({"_":"channelParticipantBanned","left":true}),
        json!({"_":"channelParticipantBanned","banned_rights":{"view_messages":true}}),
    ] {
        let m = Mock::new();
        m.reply(
            "channels.getParticipant",
            json!({"participant":participant}),
        );
        assert!(
            run(&m, "remove_user", json!({"chat_id":7,"user_id":8}))
                .await
                .is_err()
        );
        assert!(m.sent("channels.editBanned").is_empty());
    }
}

#[tokio::test]
async fn remove_user_refuses_self_identifiers_and_resolved_self_users() {
    let m = Mock::new();
    assert!(
        run(&m, "remove_user", json!({"chat_id":7,"user_id":"me"}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
    let m = Mock::new();
    m.reply("users.getUsers", json!([{"_":"user","id":"8","self":true}]));
    assert!(
        run(&m, "remove_user", json!({"chat_id":7,"user_id":8}))
            .await
            .is_err()
    );
    assert!(m.sent("channels.editBanned").is_empty());
}

#[tokio::test]
async fn remove_user_handles_basic_groups_and_honestly_reports_failed_ban_cleanup() {
    let m = Mock::new();
    run(&m, "remove_user", json!({"chat_id":9,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(m.sent("messages.deleteChatUser")[0]["chat_id"], "9");
    let m = Mock::new();
    m.reply("channels.editBanned", json!(true));
    m.fail("channels.editBanned");
    let result = run(&m, "remove_user", json!({"chat_id":7,"user_id":8}))
        .await
        .unwrap();
    assert_eq!(result["kicked"], true);
    assert_eq!(result["ban_lifted"], false);
    assert!(result["warning"].as_str().unwrap().contains("unban_user"));
}

#[tokio::test]
async fn default_permissions_preserves_individual_bits_upstream_defaults_and_expiry() {
    let m = Mock::new();
    run(&m,"set_default_chat_permissions",json!({"chat_id":7,"send_stickers":false,"send_gifs":true,"send_games":false,"send_inline":true,"until_date":1900000000})).await.unwrap();
    let rights = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    for (field, banned) in [
        ("send_stickers", true),
        ("send_gifs", false),
        ("send_games", true),
        ("send_inline", false),
        ("send_messages", false),
        ("change_info", true),
        ("pin_messages", true),
    ] {
        assert_eq!(rights[field], banned, "{field}");
    }
    assert_eq!(rights["until_date"], 1900000000);
}

#[tokio::test]
async fn default_permissions_rejects_invalid_expiry_and_types_before_network() {
    for args in [
        json!({"chat_id":7,"until_date":-1}),
        json!({"chat_id":7,"until_date":2147483648i64}),
        json!({"chat_id":7,"send_gifs":"yes"}),
    ] {
        let m = Mock::new();
        assert!(run(&m, "set_default_chat_permissions", args).await.is_err());
        assert!(m.methods().is_empty());
    }
}

#[test]
fn new_identity_metadata_has_real_routes_and_explicit_effect_annotations() {
    let all: Vec<Value> =
        serde_json::from_str(include_str!("../../resources/tool-catalogue.json")).unwrap();
    let added = [
        "resolve_phone",
        "mark_topic_as_read",
        "upgrade_group_to_supergroup",
        "promote_admin",
        "remove_user",
        "set_default_chat_permissions",
        "edit_admin_rights",
    ];
    let metadata: Vec<_> = all
        .iter()
        .filter(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| added.contains(&name))
        })
        .collect();
    assert_eq!(metadata.len(), added.len());
    for tool in metadata {
        let name = tool["name"].as_str().unwrap();
        assert!(telegram_mcp::tools::identity::TOOL_NAMES.contains(&name));
        assert_eq!(tool["annotations"]["readOnlyHint"], name == "resolve_phone");
        assert_eq!(
            tool["annotations"]["destructiveHint"],
            name != "resolve_phone"
        );
    }
    let names = telegram_mcp::tools::identity::names();
    assert_eq!(names.len(), telegram_mcp::tools::identity::TOOL_NAMES.len());
    assert_eq!(
        names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        names.len()
    );
}
