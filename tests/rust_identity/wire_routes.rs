use super::common::*;
use serde_json::{Value, json};

// These exercise distinct public adapters through the real TL encoder. A route
// accidentally wired to a similar method or wrong peer constructor must fail.
macro_rules! route {
    ($name:ident,$args:expr,$method:literal) => {
        route!($name, $args, $method, |_: &Value| {});
    };
    ($name:ident,$args:expr,$method:literal,$check:expr) => {
        #[tokio::test]
        async fn $name() {
            let m = Mock::new();
            run(&m, stringify!($name), $args).await.unwrap();
            assert_eq!(m.methods().last().map(String::as_str), Some($method));
            let calls = m.sent($method);
            let request = calls.last().unwrap();
            let schema = telegram_mcp::tl::Schema::global();
            let decoded = schema
                .decode("Object", &schema.encode_call($method, request).unwrap())
                .unwrap();
            ($check)(&decoded);
        }
    };
}
route!(search_contacts, json!({"query":"Ada"}), "contacts.search");
route!(get_contact_ids, json!({}), "contacts.getContactIDs");
route!(get_blocked_users, json!({}), "contacts.getBlocked");
route!(
    import_contacts,
    json!({"contacts":[{"phone":"+123","first_name":"Ada","last_name":"Lovelace"}]}),
    "contacts.importContacts"
);
route!(export_contacts, json!({}), "contacts.getContacts");
route!(block_user, json!({"user_id":8}), "contacts.block");
route!(unblock_user, json!({"user_id":8}), "contacts.unblock");
route!(get_chat, json!({"chat_id":7}), "messages.getPeerDialogs");
route!(get_full_chat, json!({"chat_id":9}), "messages.getFullChat");
#[tokio::test]
async fn resolve_username() {
    for (input, returned, accepted) in [
        (
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
            json!({"_":"peerUser","user_id":8}),
            true,
        ),
        (
            json!({"_":"inputPeerChannel","channel_id":"8","access_hash":"80"}),
            json!({"_":"peerChannel","channel_id":8}),
            true,
        ),
        (
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
            json!({"_":"peerUser","user_id":"9"}),
            false,
        ),
        (
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
            json!({"_":"peerChannel","channel_id":"8"}),
            false,
        ),
    ] {
        let m = Mock::new();
        m.set_peer("@Ada", input);
        let response = json!({"_":"contacts.resolvedPeer","peer":returned,"chats":[],"users":[]});
        m.reply("contacts.resolveUsername", response.clone());
        let result = run(&m, "resolve_username", json!({"username":"@Ada"})).await;
        if accepted {
            assert_eq!(result.unwrap(), response);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Username identity changed")
            );
        }
        assert_eq!(m.resolutions(), [json!("@Ada")]);
        assert_eq!(m.methods(), ["contacts.resolveUsername"]);
        let request = &m.sent("contacts.resolveUsername")[0];
        let schema = telegram_mcp::tl::Schema::global();
        let decoded = schema
            .decode(
                "Object",
                &schema
                    .encode_call("contacts.resolveUsername", request)
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(decoded["username"], "Ada");
    }
}
route!(
    search_public_chats,
    json!({"query":"news","kind":"channels"}),
    "contacts.search"
);
route!(
    get_common_chats,
    json!({"user_id":8,"max_id":"9007199254740993","limit":1000}),
    "messages.getCommonChats"
);
route!(
    get_message_link,
    json!({"chat_id":7,"message_id":12,"thread":true}),
    "channels.exportMessageLink"
);
route!(
    pin_chat,
    json!({"chat":7}),
    "messages.toggleDialogPin",
    |r: &Value| {
        assert_eq!(r["peer"]["peer"]["channel_id"], "7");
        assert_eq!(r["pinned"], true);
    }
);
route!(
    unpin_chat,
    json!({"chat":7}),
    "messages.toggleDialogPin",
    |r: &Value| {
        assert_eq!(r["peer"]["peer"]["channel_id"], "7");
        assert_eq!(r["pinned"], false);
    }
);
route!(
    mark_chat_unread,
    json!({"chat":7}),
    "messages.markDialogUnread"
);
route!(
    subscribe_public_channel,
    json!({"channel":7}),
    "channels.joinChannel"
);
route!(
    archive_chat,
    json!({"chat_id":7}),
    "folders.editPeerFolders",
    |r: &Value| {
        assert_eq!(r["folder_peers"][0]["peer"]["channel_id"], "7");
        assert_eq!(r["folder_peers"][0]["folder_id"], 1);
    }
);
route!(
    unarchive_chat,
    json!({"chat_id":7}),
    "folders.editPeerFolders",
    |r: &Value| {
        assert_eq!(r["folder_peers"][0]["peer"]["channel_id"], "7");
        assert_eq!(r["folder_peers"][0]["folder_id"], 0);
    }
);
route!(
    create_group,
    json!({"title":"Friends","user_ids":[8]}),
    "messages.createChat"
);
route!(
    edit_chat_title,
    json!({"chat_id":9,"title":"Friends"}),
    "messages.editChatTitle"
);
route!(
    edit_chat_photo,
    json!({"chat_id":7,"file_path":"photo.png"}),
    "channels.editPhoto"
);
route!(
    edit_chat_about,
    json!({"chat_id":7,"about":"About"}),
    "messages.editChatAbout"
);
route!(
    delete_chat_photo,
    json!({"chat_id":9}),
    "messages.editChatPhoto"
);
route!(
    toggle_slow_mode,
    json!({"chat_id":7,"seconds":60}),
    "channels.toggleSlowMode"
);
route!(leave_chat, json!({"chat_id":9}), "messages.deleteChatUser");
route!(
    invite_to_group,
    json!({"group_id":7,"user_ids":[8]}),
    "channels.inviteToChannel"
);
route!(
    get_participants,
    json!({"chat_id":7,"page":2,"page_size":200}),
    "channels.getParticipants"
);
route!(
    get_banned_users,
    json!({"chat_id":7}),
    "channels.getParticipants"
);
route!(get_admins, json!({"chat_id":7}), "channels.getParticipants");
route!(
    get_member_admin_status,
    json!({"chat_id":7,"user_id":8}),
    "channels.getParticipant"
);
route!(
    set_member_tag,
    json!({"chat":7,"tag":"Admin","user":8}),
    "messages.editChatParticipantRank"
);
route!(
    ban_user,
    json!({"chat_id":7,"user_id":8}),
    "channels.editBanned",
    |r: &Value| {
        assert_eq!(r["channel"]["channel_id"], "7");
        assert_eq!(r["participant"]["user_id"], "8");
        assert_eq!(r["banned_rights"]["view_messages"], true);
    }
);
route!(
    unban_user,
    json!({"chat_id":7,"user_id":8}),
    "channels.editBanned",
    |r: &Value| {
        assert_eq!(r["channel"]["channel_id"], "7");
        assert_eq!(r["participant"]["user_id"], "8");
        assert_eq!(r["banned_rights"]["view_messages"], false);
    }
);
route!(
    get_recent_actions,
    json!({"chat_id":7,"query":"user","event_types":["ban","unban"],"admins":[8]}),
    "channels.getAdminLog"
);
route!(
    demote_admin,
    json!({"group_id":7,"user_id":8}),
    "channels.getParticipant"
);
route!(
    remove_member_exception,
    json!({"chat_id":7,"user_id":8}),
    "channels.editBanned"
);
route!(
    unmute_chat,
    json!({"chat_id":7}),
    "account.updateNotifySettings"
);
route!(
    get_invite_link,
    json!({"chat_id":7}),
    "channels.getFullChannel"
);
route!(
    list_invite_links,
    json!({"chat_id":7,"admin":8,"revoked":true,"limit":1000}),
    "messages.getExportedChatInvites"
);
route!(
    list_join_requests,
    json!({"chat_id":7,"link":"https://t.me/+abc"}),
    "messages.getChatInviteImporters"
);
route!(
    approve_join_request,
    json!({"chat_id":7,"user_id":8,"approved":false}),
    "messages.hideChatJoinRequest"
);
route!(
    list_invite_link_admins,
    json!({"chat_id":7}),
    "messages.getAdminsWithInvites"
);
route!(
    list_link_joins,
    json!({"chat_id":7,"link":"https://t.me/+abc","expired_subscriptions":true}),
    "messages.getChatInviteImporters"
);
route!(
    delete_revoked_invite_links,
    json!({"chat_id":7}),
    "messages.deleteRevokedExportedChatInvites"
);
route!(
    set_join_to_send,
    json!({"chat_id":7,"enabled":true}),
    "channels.toggleJoinToSend"
);
route!(
    set_join_request,
    json!({"chat_id":7,"enabled":false}),
    "channels.toggleJoinRequest"
);
route!(
    set_prehistory_hidden,
    json!({"chat_id":7,"hidden":true}),
    "channels.togglePreHistoryHidden"
);
route!(
    set_participants_hidden,
    json!({"chat_id":7,"hidden":false}),
    "channels.toggleParticipantsHidden"
);
route!(
    set_view_forum_as_messages,
    json!({"chat_id":7,"enabled":true}),
    "channels.toggleViewForumAsMessages"
);
route!(
    toggle_channel_username,
    json!({"chat_id":7,"username":"@name","active":false}),
    "channels.toggleUsername"
);
route!(
    deactivate_channel_usernames,
    json!({"chat_id":7}),
    "channels.deactivateAllUsernames"
);
route!(
    list_discussion_candidates,
    json!({}),
    "channels.getGroupsForDiscussion"
);
route!(
    set_anti_spam,
    json!({"chat_id":7,"enabled":true}),
    "channels.toggleAntiSpam"
);
route!(
    report_anti_spam_false_positive,
    json!({"chat_id":7,"message_id":12}),
    "channels.reportAntiSpamFalsePositive"
);
route!(
    set_boosts_to_unblock,
    json!({"chat_id":7,"boosts":5}),
    "channels.setBoostsToUnblockRestrictions"
);
route!(
    set_channel_color,
    json!({"chat_id":7,"color":3,"background_emoji_id":"9007199254740993","for_profile":true}),
    "channels.updateColor"
);
route!(
    set_channel_stickers,
    json!({"chat_id":7,"set_short_name":"https://t.me/addstickers/Pack"}),
    "channels.setStickers"
);
route!(
    set_channel_emoji_pack,
    json!({"chat_id":7,"set_short_name":null}),
    "channels.setEmojiStickers"
);
route!(
    convert_to_gigagroup,
    json!({"chat_id":7}),
    "channels.convertToGigagroup"
);
route!(
    delete_channel,
    json!({"chat_id":7}),
    "channels.deleteChannel"
);
route!(
    set_channel_location,
    json!({"chat_id":7,"latitude":35.0,"longitude":51.0,"address":"Place"}),
    "channels.editLocation"
);
route!(
    set_autotranslation,
    json!({"chat_id":7,"enabled":true}),
    "channels.toggleAutotranslation"
);
route!(
    get_similar_channels,
    json!({"chat_id":7}),
    "channels.getChannelRecommendations"
);
route!(
    list_my_communities,
    json!({}),
    "communities.getJoinedCommunities"
);
route!(
    get_community_chats,
    json!({"community":7}),
    "channels.getFullChannel"
);
route!(
    create_community,
    json!({"first_chat":9,"title":"Friends","hidden":true}),
    "communities.create"
);
route!(
    set_community_photo,
    json!({"community":7,"file_path":"photo.png"}),
    "channels.editPhoto"
);
route!(
    delete_community_photo,
    json!({"community":7}),
    "channels.editPhoto"
);
route!(
    delete_community,
    json!({"community":7}),
    "channels.deleteChannel"
);
route!(
    add_chat_to_community,
    json!({"community":7,"chat":9,"visibility":"hidden"}),
    "communities.togglePeerLink"
);
route!(
    remove_chat_from_community,
    json!({"community":7,"chat":9}),
    "communities.togglePeerLink"
);
route!(
    get_community_link_requests,
    json!({"community":7,"limit":1000}),
    "communities.getPeerLinkRequests"
);
route!(
    approve_community_link_request,
    json!({"community":7,"all":true}),
    "communities.toggleAllPeerLinkRequestApproval"
);
route!(
    reject_community_link_request,
    json!({"community":7,"chat":9}),
    "communities.togglePeerLinkRequestApproval"
);
route!(
    ban_community_member,
    json!({"community":7,"user":8}),
    "communities.toggleParticipantBanned"
);
route!(
    unban_community_member,
    json!({"community":7,"user":8}),
    "communities.toggleParticipantBanned"
);
route!(get_me, json!({}), "users.getFullUser");
route!(
    update_profile,
    json!({"first_name":"Ada","last_name":"Lovelace","about":"About"}),
    "account.updateProfile"
);
route!(get_full_user, json!({"username":8}), "users.getFullUser");
route!(
    get_bot_info,
    json!({"bot_username":"8"}),
    "users.getFullUser"
);
route!(
    set_bot_commands,
    json!({"commands":[{"command":"start","description":"Start"}]}),
    "bots.setBotCommands"
);
route!(get_bot_commands, json!({}), "bots.getBotCommands");
route!(
    get_user_photos,
    json!({"user_id":8,"limit":1000}),
    "photos.getUserPhotos"
);
route!(get_user_status, json!({"user_id":8}), "users.getUsers");
route!(
    set_bot_info,
    json!({"bot":8,"name":"Bot","about":"About","description":"Description","lang_code":"en"}),
    "bots.setBotInfo"
);
route!(
    set_my_username,
    json!({"username":"@new_name"}),
    "account.updateUsername"
);
route!(list_authorizations, json!({}), "account.getAuthorizations");
route!(
    set_authorization_secret_chats,
    json!({"hash":"8","accept_secret_chats":false}),
    "account.changeAuthorizationSettings"
);

#[tokio::test]
async fn replace_chat_photo_without_file_keeps_current_and_deletes_only_older_photo_actions() {
    let m = Mock::new();
    m.reply("messages.search",json!({"messages":[{"id":30,"action":{"_":"messageActionChatEditPhoto"}},{"id":20,"action":{"_":"messageActionChatDeletePhoto"}},{"id":10,"action":{"_":"messageActionChatEditPhoto"}}]}));
    let r = run(&m, "replace_chat_photo", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(r["current_photo_retained"], true);
    assert_eq!(m.sent("channels.deleteMessages")[0]["id"], json!([10]));
    assert!(m.sent("channels.editPhoto").is_empty());
}
