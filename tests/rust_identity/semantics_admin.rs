use super::common::*;
use serde_json::json;

#[tokio::test]
async fn creator_can_patch_only_anonymous_and_title_while_retaining_other_rights() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantCreator","is_self":true,"rank":"Owner","admin_rights":{"_":"chatAdminRights","ban_users":true,"manage_welcome_messages":true}}}));
    let result = run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"mode":"patch","anonymous":true,"rank":"Quiet owner"}),
    )
    .await
    .unwrap();
    let sent = &m.sent("channels.editAdmin")[0];
    assert_eq!(sent["rank"], "Quiet owner");
    assert_eq!(sent["admin_rights"]["anonymous"], true);
    assert_eq!(sent["admin_rights"]["ban_users"], true);
    assert_eq!(sent["admin_rights"]["manage_welcome_messages"], true);
    assert_eq!(result["accepted"], true);
    assert_eq!(m.sent("channels.getParticipant").len(), 2);
}

#[tokio::test]
async fn creator_cannot_use_patch_mode_to_change_their_inherent_moderation_rights() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantCreator","is_self":true,"admin_rights":{"_":"chatAdminRights","ban_users":true}}}));
    assert!(
        run(
            &m,
            "edit_admin_rights_group",
            json!({"group_id":7,"user_id":8,"mode":"patch","ban_users":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}

#[tokio::test]
async fn identical_admin_patch_stops_after_the_read_without_repeating_a_write() {
    let m = Mock::new();
    let result = run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"mode":"patch","ban_users":true,"rank":"Keeper"}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"unchanged":true}));
    assert!(m.sent("channels.editAdmin").is_empty());
    assert_eq!(m.sent("channels.getParticipant").len(), 1);
}

#[tokio::test]
async fn clearing_a_title_in_patch_mode_keeps_all_unmentioned_rights() {
    let m = Mock::new();
    run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"mode":"patch","rank":""}),
    )
    .await
    .unwrap();
    let sent = &m.sent("channels.editAdmin")[0];
    assert_eq!(sent["rank"], "");
    assert_eq!(sent["admin_rights"]["ban_users"], true);
    assert_eq!(sent["admin_rights"]["delete_messages"], true);
}

#[tokio::test]
async fn accepted_admin_changes_report_when_the_readback_cannot_verify_them() {
    let m = Mock::new();
    m.fail("channels.getParticipant");
    let result = run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"delete_messages":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["accepted"], true);
    assert_eq!(result["verified"], false);
    assert!(result["declined"].is_null());
    assert_eq!(m.sent("channels.editAdmin").len(), 1);
}

#[tokio::test]
async fn admin_readback_identifies_requested_flags_that_telegram_did_not_grant() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantAdmin","admin_rights":{"_":"chatAdminRights","delete_messages":true}}}));
    let result = run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"delete_messages":true,"ban_users":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["declined"], json!(["ban_users"]));
    assert_eq!(result["verified"], true);
    assert_eq!(m.sent("channels.editAdmin").len(), 1);
}

#[tokio::test]
async fn malformed_mode_or_overlong_rank_is_refused_before_looking_up_any_peer() {
    for fields in [
        json!({"mode":"merge"}),
        json!({"rank":"12345678901234567"}),
        json!({"rank":7}),
    ] {
        let m = Mock::new();
        let mut args = json!({"group_id":7,"user_id":8,"delete_messages":true});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(run(&m, "edit_admin_rights_group", args).await.is_err());
        assert!(m.methods().is_empty());
        assert!(m.resolutions().is_empty());
    }
}

#[tokio::test]
async fn group_and_channel_admin_tools_refuse_cross_type_privilege_changes() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","broadcast":true}));
    assert!(
        run(&m, "promote_admin_group", json!({"group_id":7,"user_id":8}))
            .await
            .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
    let m = Mock::new();
    assert!(
        run(
            &m,
            "promote_admin_channel",
            json!({"channel_id":7,"user_id":8})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}

#[tokio::test]
async fn channel_admin_cannot_be_given_group_only_pins_or_anonymous_flags() {
    for field in ["pin_messages", "anonymous"] {
        let m = Mock::new();
        m.info(json!({"_":"channel","id":"7","broadcast":true}));
        let mut args = json!({"channel_id":7,"user_id":8});
        args[field] = true.into();
        assert!(run(&m, "promote_admin_channel", args).await.is_err());
        assert!(m.sent("channels.editAdmin").is_empty());
    }
}

#[tokio::test]
async fn forum_privileges_are_refused_for_groups_without_topics() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":false}));
    assert!(
        run(
            &m,
            "edit_admin_rights_group",
            json!({"group_id":7,"user_id":8,"manage_topics":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}

#[tokio::test]
async fn unknown_participant_constructor_cannot_authorize_an_admin_patch() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantUnknownFutureType","admin_rights":{"_":"chatAdminRights","ban_users":true}}}));
    assert!(
        run(
            &m,
            "edit_admin_rights_group",
            json!({"group_id":7,"user_id":8,"mode":"patch","delete_messages":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}
