use super::common::*;
use serde_json::json;

#[tokio::test]
async fn member_exceptions_refuse_group_only_settings_before_reading_or_changing_expiry() {
    for setting in [
        json!({"slow_mode":"off"}),
        json!({"do_not_restrict_boosters":1}),
        json!({"charge_stars_per_message":4}),
    ] {
        let m = Mock::new();
        let mut args = json!({"chat_id":7,"user_id":8});
        args.as_object_mut()
            .unwrap()
            .extend(setting.as_object().unwrap().clone());
        assert!(run(&m, "set_member_exception", args).await.is_err());
        assert!(m.methods().is_empty());
    }
}

macro_rules! member_role{
    ($name:ident,$participant:expr,$role:literal,$admin:literal)=>{
        #[tokio::test]async fn $name(){let m=Mock::new();m.reply("channels.getParticipant",json!({"participant":$participant}));let r=run(&m,"get_member_admin_status",json!({"chat_id":7,"user_id":8})).await.unwrap();assert_eq!(r["results"][0]["role"],$role);assert_eq!(r["results"][0]["is_admin"],$admin);}
    }
}
member_role!(
    member_creator,
    json!({"_":"channelParticipantCreator","user_id":"8","admin_rights":{"_":"chatAdminRights","ban_users":true}}),
    "creator",
    true
);
member_role!(
    member_admin,
    json!({"_":"channelParticipantAdmin","user_id":"8"}),
    "admin",
    true
);
member_role!(
    member_ordinary,
    json!({"_":"channelParticipant","user_id":"8"}),
    "member",
    false
);
member_role!(
    member_self,
    json!({"_":"channelParticipantSelf","user_id":"8"}),
    "member",
    false
);
member_role!(
    member_left,
    json!({"_":"channelParticipantLeft","user_id":"8"}),
    "left",
    false
);
member_role!(
    member_banned,
    json!({"_":"channelParticipantBanned","user_id":"8","banned_rights":{"view_messages":true}}),
    "banned",
    false
);
member_role!(
    member_restricted,
    json!({"_":"channelParticipantBanned","user_id":"8","banned_rights":{"send_photos":true}}),
    "restricted",
    false
);
member_role!(
    restricted_left,
    json!({"_":"channelParticipantBanned","user_id":"8","left":true,"banned_rights":{"send_photos":true}}),
    "left",
    false
);

#[tokio::test]
async fn admin_listing_preserves_user_names_roles_and_wire_rights_without_inventing_creator_rights()
{
    let m = Mock::new();
    m.reply("channels.getParticipants",json!({"participants":[{"_":"channelParticipantCreator","user_id":"8","rank":"Owner","admin_rights":{"_":"chatAdminRights","ban_users":true}}],"users":[{"_":"user","id":"8","first_name":"Ada","last_name":null}]}));
    let r = run(&m, "get_admins", json!({"chat_id":7})).await.unwrap();
    let row = &r["results"][0];
    assert_eq!(row["name"], "Ada");
    assert_eq!(row["role"], "creator");
    assert_eq!(row["rights"]["ban_users"], true);
    assert_eq!(row["rights"]["add_admins"], false);
    assert_eq!(row["rank"], "Owner");
    assert_eq!(r["complete"], true);
}

#[tokio::test]
async fn basic_group_participant_paging_selects_members_from_full_chat() {
    let m = Mock::new();
    m.reply("messages.getFullChat",json!({"full_chat":{"participants":{"participants":[{"_":"chatParticipantCreator","user_id":"8"},{"_":"chatParticipant","user_id":"10"}]}},"users":[{"id":"8","first_name":"Ada"},{"id":"10","first_name":"Bob"}]}));
    let r = run(
        &m,
        "get_participants",
        json!({"chat_id":9,"page":2,"page_size":1}),
    )
    .await
    .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["name"], "Bob");
    assert_eq!(m.methods(), ["messages.getFullChat"]);
}

#[tokio::test]
async fn create_private_channel_does_not_check_username() {
    let m = Mock::new();
    m.reply(
        "channels.createChannel",
        json!({"chats":[{"_":"channel","id":"7","access_hash":"70"}]}),
    );
    let r = run(&m, "create_channel", json!({"title":"News"}))
        .await
        .unwrap();
    assert_eq!(r["created"], true);
    assert_eq!(r["public"], false);
    assert_eq!(r["chat_id"], "-1000000000007");
    assert_eq!(m.methods(), ["channels.createChannel"]);
}
#[tokio::test]
async fn taken_public_username_prevents_channel_creation() {
    let m = Mock::new();
    m.reply("channels.checkUsername", false.into());
    assert!(
        run(
            &m,
            "create_channel",
            json!({"title":"News","username":"taken_name"})
        )
        .await
        .is_err()
    );
    assert_eq!(m.methods(), ["channels.checkUsername"]);
}
#[tokio::test]
async fn public_channel_username_is_checked_before_creation() {
    let m = Mock::new();
    m.reply(
        "channels.createChannel",
        json!({"chats":[{"_":"channel","id":"7","access_hash":"70"}]}),
    );
    let r = run(
        &m,
        "create_channel",
        json!({"title":"News","username":"@ops_room"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.methods(),
        [
            "channels.checkUsername",
            "channels.createChannel",
            "channels.updateUsername"
        ]
    );
    assert_eq!(r["public"], true);
    assert_eq!(r["link"], "https://t.me/ops_room");
}
#[tokio::test]
async fn username_race_reports_created_channel_to_avoid_duplicate() {
    let m = Mock::new();
    m.reply(
        "channels.createChannel",
        json!({"chats":[{"_":"channel","id":"7","access_hash":"70"}]}),
    );
    m.fail("channels.updateUsername");
    let r = run(
        &m,
        "create_channel",
        json!({"title":"News","username":"ops_room"}),
    )
    .await
    .unwrap();
    assert_eq!(r["created"], true);
    assert_eq!(r["public"], false);
    assert!(
        r["warning"]
            .as_str()
            .unwrap()
            .contains("Do not create another")
    );
    assert_eq!(r["chat_id"], "-1000000000007");
    m.set_peer(
        "-1000000000007",
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}),
    );
    run(
        &m,
        "set_channel_username",
        json!({"chat_id":r["chat_id"],"username":"ops_room"}),
    )
    .await
    .unwrap();
    assert_eq!(m.sent("channels.createChannel").len(), 1);
    assert_eq!(
        m.sent("channels.updateUsername")[1]["channel"]["channel_id"],
        "7"
    );
}
#[tokio::test]
async fn admin_patch_preserves_unmentioned_rights_and_title() {
    let m = Mock::new();
    run(
        &m,
        "edit_admin_rights_group",
        json!({"group_id":7,"user_id":8,"mode":"patch","delete_messages":false}),
    )
    .await
    .unwrap();
    let p = &m.sent("channels.editAdmin")[0];
    assert_eq!(p["admin_rights"]["delete_messages"], false);
    assert_eq!(p["admin_rights"]["ban_users"], true);
    assert_eq!(p["admin_rights"]["other"], true);
    assert_eq!(p["rank"], "Keeper");
}
#[tokio::test]
async fn admin_patch_cannot_demote_implicitly() {
    let m = Mock::new();
    m.reply(
        "channels.getParticipant",
        json!({"participant":{"_":"channelParticipantAdmin","admin_rights":{"_":"chatAdminRights","delete_messages":true}}}),
    );
    assert!(
        run(
            &m,
            "edit_admin_rights_group",
            json!({"group_id":7,"user_id":8,"mode":"patch","delete_messages":false})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}
#[tokio::test]
async fn admin_patch_of_nonadmin_is_refused() {
    let m = Mock::new();
    m.reply("channels.getParticipant", json!({"participant":{}}));
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
#[tokio::test]
async fn admin_patch_of_self_noncreator_is_refused() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"_":"channelParticipantAdmin","is_self":true,"admin_rights":{"_":"chatAdminRights","ban_users":true}}}));
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
#[tokio::test]
async fn group_admin_cannot_be_given_channel_only_right() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "promote_admin_group",
            json!({"group_id":7,"user_id":8,"post_messages":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editAdmin").is_empty());
}
#[tokio::test]
async fn promote_group_defaults_full_but_not_anonymous() {
    let m = Mock::new();
    run(&m, "promote_admin_group", json!({"group_id":7,"user_id":8}))
        .await
        .unwrap();
    let r = &m.sent("channels.editAdmin")[0]["admin_rights"];
    assert_eq!(r["anonymous"], false);
    assert_eq!(r["ban_users"], true);
    assert!(r.get("post_messages").is_none());
}
#[tokio::test]
async fn permission_patch_preserves_untouched_restrictions() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true,"default_banned_rights":{"_":"chatBannedRights","until_date":0,"send_photos":true}}));
    run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"polls":false}),
    )
    .await
    .unwrap();
    let r = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    assert_eq!(r["send_photos"], true);
    assert_eq!(r["send_polls"], true);
    assert!(r.get("send_media").is_none());
}
#[tokio::test]
async fn denying_messages_also_denies_embed_links() {
    let m = Mock::new();
    run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"send_messages":false}),
    )
    .await
    .unwrap();
    let r = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    assert_eq!(r["send_plain"], true);
    assert_eq!(r["embed_links"], true);
}
#[tokio::test]
async fn permissions_partial_failure_reports_already_applied_steps() {
    let m = Mock::new();
    m.fail("channels.toggleSlowMode");
    let r = run(
        &m,
        "set_group_permissions",
        json!({"chat_id":7,"polls":false,"slow_mode":"1m"}),
    )
    .await
    .unwrap();
    assert_eq!(r["complete"], false);
    assert_eq!(r["applied"], json!(["Permissions"]));
    assert_eq!(r["failed"], "Slow mode");
}
#[tokio::test]
async fn member_exception_does_not_turn_temporary_into_forever() {
    let m = Mock::new();
    m.reply("channels.getParticipant",json!({"participant":{"banned_rights":{"_":"chatBannedRights","until_date":2000000000,"send_photos":true}}}));
    run(
        &m,
        "set_member_exception",
        json!({"chat_id":7,"user_id":8,"polls":false}),
    )
    .await
    .unwrap();
    let p = &m.sent("channels.editBanned")[0]["banned_rights"];
    assert_eq!(p["until_date"], 2000000000_i64);
    assert_eq!(p["send_photos"], true);
}
#[tokio::test]
async fn group_denial_cannot_be_lifted_for_one_member() {
    let m = Mock::new();
    m.info(json!({"_":"channel","id":"7","megagroup":true,"forum":true,"default_banned_rights":{"_":"chatBannedRights","until_date":0,"send_photos":true}}));
    assert!(
        run(
            &m,
            "set_member_exception",
            json!({"chat_id":7,"user_id":8,"photos":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("channels.editBanned").is_empty());
}
