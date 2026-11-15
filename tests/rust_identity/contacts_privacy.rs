use super::common::*;
use serde_json::json;

#[tokio::test]
async fn direct_contact_chat_returns_normalized_rows_without_unrelated_raw_entities() {
    let m = Mock::new();
    m.reply(
        "contacts.getContacts",
        json!({"users":[{"id":"8","access_hash":"80","first_name":"Ada"}]}),
    );
    m.reply("messages.getPeerDialogs",json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"8"},"unread_count":7}],"users":[{"id":"99","first_name":"Unrelated"}]}));
    let r = run(
        &m,
        "get_direct_chat_by_contact",
        json!({"contact_query":"ada"}),
    )
    .await
    .unwrap();
    assert_eq!(r["results"][0]["chat_id"], "8");
    assert_eq!(r["results"][0]["contact"], "Ada");
    assert_eq!(r["results"][0]["unread"], 7);
    assert!(r.get("users").is_none());
}
#[tokio::test]
async fn common_contact_chats_return_marked_group_ids() {
    let m = Mock::new();
    m.reply("messages.getCommonChats",json!({"chats":[{"_":"channel","id":"7","title":"Shared"},{"_":"chat","id":"9","title":"Basic"}]}));
    let r = run(&m, "get_contact_chats", json!({"contact_id":8}))
        .await
        .unwrap();
    assert_eq!(r["results"][1]["chat_id"], "-1000000000007");
    assert_eq!(r["results"][2]["chat_id"], "-9");
}

#[tokio::test]
async fn contact_search_prepends_saved_aliases_without_claiming_similar_match_is_exact() {
    let m = Mock::new();
    m.reply(
        "local.alias_matches",
        json!([{"alias":"ada work","id":"8","favorite":true,"match":"similar"}]),
    );
    m.reply(
        "contacts.search",
        json!({"users":[{"id":"8","first_name":"Ada"}]}),
    );
    let r = run(&m, "search_contacts", json!({"query":"ada"}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["favorite"], true);
    assert_eq!(r["results"][0]["match"], "similar");
    assert_eq!(r["results"][1]["name"], "Ada");
}

#[tokio::test]
async fn contact_names_omit_null_and_identify_deleted_accounts() {
    let m = Mock::new();
    m.reply("contacts.getContacts",json!({"users":[{"id":"8","first_name":"Ada","last_name":null},{"id":"9","deleted":true,"first_name":null,"last_name":null}]}));
    let r = run(&m, "list_contacts", json!({})).await.unwrap();
    assert_eq!(r["results"][0]["name"], "Ada");
    assert_eq!(r["results"][1]["name"], "Deleted Account");
    assert_eq!(m.methods(), ["contacts.getContacts"]);
}
#[tokio::test]
async fn add_username_contact_uses_resolved_input_user() {
    let m = Mock::new();
    m.reply("contacts.resolveUsername",json!({"peer":{"_":"peerUser","user_id":"8"},"users":[{"_":"user","id":"8","access_hash":"80"}]}));
    run(
        &m,
        "add_contact",
        json!({"username":"@bob","first_name":"Ada","last_name":"Lovelace"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.methods(),
        ["contacts.resolveUsername", "contacts.addContact"]
    );
    assert_eq!(m.sent("contacts.resolveUsername")[0]["username"], "bob");
    let r = &m.sent("contacts.addContact")[0];
    assert_eq!(r["id"]["_"], "inputUser");
    assert_eq!(r["id"]["user_id"], "8");
    assert_eq!(r["phone"], "");
}
#[tokio::test]
async fn add_phone_contact_imports_typed_contact_without_username_lookup() {
    let m = Mock::new();
    run(
        &m,
        "add_contact",
        json!({"phone":"+15550100","first_name":"Ada"}),
    )
    .await
    .unwrap();
    assert_eq!(m.methods(), ["contacts.importContacts"]);
    let c = &m.sent("contacts.importContacts")[0]["contacts"][0];
    assert_eq!(c["_"], "inputPhoneContact");
    assert_eq!(c["phone"], "+15550100");
}
#[tokio::test]
async fn add_contact_requires_identifier_before_network() {
    let m = Mock::new();
    assert!(run(&m, "add_contact", json!({})).await.is_err());
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn resolved_channel_cannot_be_added_as_contact() {
    let m = Mock::new();
    m.reply(
        "contacts.resolveUsername",
        json!({"peer":{"_":"peerChannel","channel_id":"7"},"users":[]}),
    );
    assert!(
        run(&m, "add_contact", json!({"username":"group"}))
            .await
            .is_err()
    );
    assert!(m.sent("contacts.addContact").is_empty());
}
#[tokio::test]
async fn contact_chats_gets_direct_dialog_without_listing_every_dialog() {
    let m = Mock::new();
    m.reply("messages.getCommonChats", json!({"chats":[]}));
    let r = run(&m, "get_contact_chats", json!({"contact_id":8}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["type"], "Private");
    assert_eq!(
        m.methods(),
        ["messages.getPeerDialogs", "messages.getCommonChats"]
    );
}
#[tokio::test]
async fn reading_last_interaction_does_not_mark_seen() {
    let m = Mock::new();
    run(&m, "get_last_interaction", json!({"contact_id":8}))
        .await
        .unwrap();
    assert_eq!(m.methods(), ["messages.getHistory"]);
    assert_eq!(m.sent("messages.getHistory")[0]["limit"], 5);
}
#[tokio::test]
async fn deleting_contact_uses_input_user_vector() {
    let m = Mock::new();
    run(&m, "delete_contact", json!({"user_id":8}))
        .await
        .unwrap();
    assert_eq!(
        m.sent("contacts.deleteContacts")[0]["id"][0]["_"],
        "inputUser"
    );
}
#[tokio::test]
async fn contact_send_is_real_media_send_with_random_id() {
    let m = Mock::new();
    run(
        &m,
        "send_contact",
        json!({"chat_id":7,"phone_number":"+123","first_name":"Ada"}),
    )
    .await
    .unwrap();
    let p = &m.sent("messages.sendMedia")[0];
    assert_eq!(p["media"]["_"], "inputMediaContact");
    assert!(p.get("random_id").is_some());
}

macro_rules! privacy_key_tests{($read:ident,$write:ident,$key:literal,$kind:literal)=>{
    #[tokio::test]async fn $read(){let m=Mock::new();m.reply("account.getPrivacy",json!({"rules":[]}));run(&m,"get_privacy_settings",json!({"key":$key})).await.unwrap();assert_eq!(m.sent("account.getPrivacy")[0]["key"]["_"],$kind);}
    #[tokio::test]async fn $write(){let m=Mock::new();run(&m,"set_privacy_settings",json!({"key":$key,"base_policy":"contacts"})).await.unwrap();assert_eq!(m.sent("account.setPrivacy")[0]["key"]["_"],$kind);assert_eq!(m.sent("account.setPrivacy")[0]["rules"][0]["_"],"inputPrivacyValueAllowContacts");}
}}
privacy_key_tests!(
    read_status,
    write_status,
    "status",
    "inputPrivacyKeyStatusTimestamp"
);
privacy_key_tests!(
    read_phone,
    write_phone,
    "phone",
    "inputPrivacyKeyPhoneNumber"
);
privacy_key_tests!(
    read_photo,
    write_photo,
    "profile_photo",
    "inputPrivacyKeyProfilePhoto"
);
privacy_key_tests!(
    read_invite,
    write_invite,
    "chat_invite",
    "inputPrivacyKeyChatInvite"
);
privacy_key_tests!(
    read_call,
    write_call,
    "phone_call",
    "inputPrivacyKeyPhoneCall"
);
privacy_key_tests!(read_p2p, write_p2p, "phone_p2p", "inputPrivacyKeyPhoneP2P");
privacy_key_tests!(
    read_forwards,
    write_forwards,
    "forwards",
    "inputPrivacyKeyForwards"
);
privacy_key_tests!(
    read_voice,
    write_voice,
    "voice_messages",
    "inputPrivacyKeyVoiceMessages"
);
privacy_key_tests!(read_about, write_about, "about", "inputPrivacyKeyAbout");
privacy_key_tests!(
    read_birthday,
    write_birthday,
    "birthday",
    "inputPrivacyKeyBirthday"
);
privacy_key_tests!(
    read_find_phone,
    write_find_phone,
    "added_by_phone",
    "inputPrivacyKeyAddedByPhone"
);
privacy_key_tests!(
    read_music,
    write_music,
    "saved_music",
    "inputPrivacyKeySavedMusic"
);

#[tokio::test]
async fn privacy_requires_explicit_base_before_sending_anything() {
    let m = Mock::new();
    assert!(
        run(&m, "set_privacy_settings", json!({"key":"status"}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn privacy_exceptions_precede_base_policy_and_use_input_users() {
    let m = Mock::new();
    run(
        &m,
        "set_privacy_settings",
        json!({"key":"status","base_policy":"nobody","allow_users":[8]}),
    )
    .await
    .unwrap();
    let p = &m.sent("account.setPrivacy")[0];
    assert_eq!(p["rules"][0]["_"], "inputPrivacyValueAllowUsers");
    assert_eq!(p["rules"][0]["users"][0]["_"], "inputUser");
    assert_eq!(p["rules"][1]["_"], "inputPrivacyValueDisallowAll");
}
#[tokio::test]
async fn privacy_rejects_conflicting_exceptions() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_privacy_settings",
            json!({"key":"status","base_policy":"contacts","allow_users":[8],"disallow_users":[8]})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn privacy_roundtrip_keeps_close_friends_and_order() {
    let m = Mock::new();
    run(&m,"set_privacy_settings",json!({"key":"status","rules":[{"rule":"close_friends_allowed"},{"rule":"users_disallowed","users":[8]},{"rule":"contacts_allowed"}]})).await.unwrap();
    let rules = &m.sent("account.setPrivacy")[0]["rules"];
    assert_eq!(rules[0]["_"], "inputPrivacyValueAllowCloseFriends");
    assert_eq!(rules[1]["_"], "inputPrivacyValueDisallowUsers");
    assert_eq!(rules[2]["_"], "inputPrivacyValueAllowContacts");
}
#[tokio::test]
async fn privacy_unknown_later_rule_prevents_all_mutations() {
    let m = Mock::new();
    assert!(run(&m,"set_privacy_settings",json!({"key":"status","rules":[{"rule":"users_allowed","users":[8]},{"rule":"not_a_rule"}]})).await.is_err());
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn privacy_empty_complete_list_is_refused() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_privacy_settings",
            json!({"key":"status","rules":[]})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
