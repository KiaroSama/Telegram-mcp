use super::common::*;
use serde_json::json;

#[tokio::test]
async fn legacy_media_denial_is_preserved_while_one_media_class_is_reenabled() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true,"default_banned_rights":{"_":"chatBannedRights","send_media":true,"until_date":0}}));
    run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"photos":true}),
    )
    .await
    .unwrap();
    let rights = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    assert!(rights.get("send_photos").is_none());
    assert_eq!(rights["send_videos"], true);
    assert_eq!(rights["send_audios"], true);
    assert_eq!(rights["send_polls"], true);
    assert!(rights.get("send_media").is_none());
}

#[tokio::test]
async fn explicit_media_class_overrides_the_coarse_send_media_switch() {
    let m = Mock::new();
    let result = run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"send_media":false,"photos":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["permissions"]["photos"], true);
    assert_eq!(result["permissions"]["video_files"], false);
    let rights = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    assert!(rights.get("send_photos").is_none());
    assert_eq!(rights["send_gifs"], true);
    assert_eq!(rights["send_stickers"], true);
}

#[tokio::test]
async fn group_settings_apply_permissions_slowmode_boosters_and_price_in_order() {
    let m = Mock::new();
    let result=run(&m,"set_group_permissions",json!({"chat_id":7,"polls":false,"slow_mode":"15m","do_not_restrict_boosters":2,"charge_stars_per_message":4})).await.unwrap();
    assert_eq!(
        result["applied"],
        json!([
            "Permissions",
            "Slow mode",
            "Boost exemptions",
            "Message price"
        ])
    );
    assert_eq!(m.sent("channels.toggleSlowMode")[0]["seconds"], 900);
    assert_eq!(
        m.sent("channels.setBoostsToUnblockRestrictions")[0]["boosts"],
        2
    );
    assert_eq!(
        m.sent("channels.updatePaidMessagesPrice")[0]["send_paid_messages_stars"],
        4
    );
}

#[tokio::test]
async fn failure_in_boost_settings_does_not_proceed_to_the_message_price() {
    let m = Mock::new();
    m.fail("channels.setBoostsToUnblockRestrictions");
    let result=run(&m,"set_group_permissions",json!({"chat_id":7,"polls":false,"slow_mode":60,"do_not_restrict_boosters":1,"charge_stars_per_message":4})).await.unwrap();
    assert_eq!(result["complete"], false);
    assert_eq!(result["applied"], json!(["Permissions", "Slow mode"]));
    assert_eq!(result["failed"], "Boost exemptions");
    assert!(m.sent("channels.updatePaidMessagesPrice").is_empty());
}

#[tokio::test]
async fn booster_exemptions_require_an_actual_existing_or_requested_restriction() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_group_permissions",
            json!({"chat_id":7,"do_not_restrict_boosters":2})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.setBoostsToUnblockRestrictions").is_empty());
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true,"slowmode_enabled":true}));
    run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"do_not_restrict_boosters":2}),
    )
    .await
    .unwrap();
    assert_eq!(m.sent("channels.setBoostsToUnblockRestrictions").len(), 1);
    assert!(m.sent("messages.editChatDefaultBannedRights").is_empty());
}

#[tokio::test]
async fn malformed_permission_values_and_ranges_do_not_trigger_any_read_or_write() {
    for fields in [
        json!({"photos":"yes"}),
        json!({"send_media":"yes"}),
        json!({"slow_mode":"two minutes"}),
        json!({"slow_mode":7}),
        json!({"do_not_restrict_boosters":6}),
        json!({"charge_stars_per_message":-1}),
        json!({"until_date":-1}),
        json!({}),
    ] {
        let m = Mock::new();
        let mut args = json!({"chat_id":7});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(run(&m, "set_group_permissions", args).await.is_err());
        assert!(m.methods().is_empty());
        assert!(m.resolutions().is_empty());
    }
}

#[tokio::test]
async fn default_pin_and_info_permissions_cannot_be_granted_in_public_or_linked_groups() {
    for info in [
        json!({"_":"channel","id":"7","megagroup":true,"username":"public_group"}),
        json!({"_":"channel","id":"7","megagroup":true,"has_link":true}),
    ] {
        let m = Mock::new();
        m.info(info);
        assert!(
            run(
                &m,
                "set_group_permissions",
                json!({"chat_id":7,"pin_messages":true,"change_group_info":true})
            )
            .await
            .is_err()
        );
        assert!(m.sent("messages.editChatDefaultBannedRights").is_empty());
    }
}

#[tokio::test]
async fn forum_and_supergroup_settings_are_not_silently_applied_to_other_chat_types() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":false}));
    assert!(
        run(
            &m,
            "set_group_permissions",
            json!({"chat_id":7,"create_topics":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("messages.editChatDefaultBannedRights").is_empty());
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_group_permissions",
            json!({"chat_id":9,"slow_mode":60})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.toggleSlowMode").is_empty());
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","broadcast":true}));
    assert!(
        run(
            &m,
            "set_group_permissions",
            json!({"chat_id":7,"polls":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("messages.editChatDefaultBannedRights").is_empty());
}

#[tokio::test]
async fn changing_only_member_expiry_preserves_every_existing_restriction() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantBanned","banned_rights":{"_":"chatBannedRights","until_date":42,"send_photos":true,"send_reactions":true}}}));
    run(
        &m,
        "set_member_exception",
        json!({"chat_id":7,"user_id":8,"until_date":1900000000}),
    )
    .await
    .unwrap();
    let rights = &m.sent("channels.editBanned")[0]["banned_rights"];
    assert_eq!(rights["until_date"], 1900000000);
    assert_eq!(rights["send_photos"], true);
    assert_eq!(rights["send_reactions"], true);
}

#[tokio::test]
async fn expiry_only_changes_refuse_members_without_an_existing_exception() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_member_exception",
            json!({"chat_id":7,"user_id":8,"until_date":1900000000})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editBanned").is_empty());
}

#[tokio::test]
async fn removing_a_member_exception_does_not_unban_an_already_removed_member() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantBanned","banned_rights":{"_":"chatBannedRights","view_messages":true,"until_date":0}}}));
    assert!(
        run(
            &m,
            "remove_member_exception",
            json!({"chat_id":7,"user_id":8})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editBanned").is_empty());
}

#[tokio::test]
async fn member_exception_updates_clamp_all_allowances_to_group_denials() {
    let m = Mock::new();
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 300;
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true,"default_banned_rights":{"_":"chatBannedRights","send_photos":true,"until_date":0}}));
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantBanned","banned_rights":{"_":"chatBannedRights","send_polls":true,"until_date":expiry}}}));
    run(
        &m,
        "set_member_exception",
        json!({"chat_id":7,"user_id":8,"polls":true}),
    )
    .await
    .unwrap();
    let rights = &m.sent("channels.editBanned")[0]["banned_rights"];
    assert_eq!(rights["send_photos"], true);
    assert!(rights.get("send_polls").is_none());
    assert_eq!(rights["until_date"], expiry);
}

#[tokio::test]
async fn permission_patches_cannot_make_expired_or_near_expiry_restrictions_permanent() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for expiry in [now - 1, now + 20] {
        let m = Mock::new();
        m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true}));
        m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantBanned","banned_rights":{"_":"chatBannedRights","send_polls":true,"until_date":expiry}}}));
        let error = run(
            &m,
            "set_member_exception",
            json!({"chat_id":7,"user_id":8,"polls":false}),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("within 30 seconds"));
        assert!(m.sent("channels.editBanned").is_empty());
    }
}

#[tokio::test]
async fn unrecognized_group_metadata_cannot_authorize_a_default_permission_write() {
    let m = Mock::new();
    m.info(json!({"_":"futureChannelConstructor","id":"7","megagroup":true,"forum":true}));
    assert!(
        run(
            &m,
            "set_group_permissions",
            json!({"chat_id":7,"polls":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("messages.editChatDefaultBannedRights").is_empty());
}
