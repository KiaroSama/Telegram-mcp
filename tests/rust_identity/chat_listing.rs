use super::common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn search_my_chats_falls_back_to_contacts_missing_from_telegram_search() {
    let m = Mock::new();
    m.reply("messages.getDialogFilters", json!({"filters":[]}));
    m.reply(
        "contacts.search",
        json!({"my_results":[],"users":[],"chats":[]}),
    );
    m.reply("messages.getDialogs", json!({"dialogs":[]}));
    m.reply(
        "contacts.getContacts",
        json!({"users":[{"_":"user","id":"8","access_hash":"80","first_name":"Ada"}]}),
    );
    m.reply(
        "messages.getPeerDialogs",
        json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"8"},"unread_count":2}]}),
    );
    let r = run(&m, "search_my_chats", json!({"query":"ada"}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["name"], "Ada");
    assert_eq!(r["results"][0]["unread"], 2);
}

fn dialogs() -> Value {
    json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"8"},"unread_count":0,"unread_mark":true,"notify_settings":{"mute_until":0}}, {"_":"dialog","peer":{"_":"peerChannel","channel_id":"7"},"unread_count":10,"folder_id":1,"notify_settings":{"mute_until":2000000000}}],"users":[{"_":"user","id":"8","access_hash":"80","first_name":"Ada","last_name":null,"contact":true}],"chats":[{"_":"channel","id":"7","access_hash":"70","title":"News","broadcast":true}],"messages":[]})
}
#[tokio::test]
async fn chat_list_honors_manual_unread_mark_and_mute_filter() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    let r = run(
        &m,
        "list_chats",
        json!({"unread_only":true,"unmuted_only":true}),
    )
    .await
    .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["name"], "Ada");
    assert_eq!(r["results"][0]["unread_mark"], true);
    assert_eq!(m.methods(), ["messages.getDialogs"]);
}
#[tokio::test]
async fn archived_chat_list_sets_server_folder_and_post_filters_pins() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    let r = run(&m, "list_chats", json!({"archived":true}))
        .await
        .unwrap();
    assert_eq!(m.sent("messages.getDialogs")[0]["folder_id"], 1);
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["chat_id"], "-1000000000007");
}
#[tokio::test]
async fn about_fetch_is_opt_in_and_user_bio_is_preserved() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    m.reply("users.getFullUser", json!({"full_user":{"about":"زندگی"}}));
    m.reply(
        "channels.getFullChannel",
        json!({"full_chat":{"about":"News description"}}),
    );
    let r = run(&m, "list_chats", json!({"with_about":true}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["about"], "زندگی");
    assert_eq!(r["results"][1]["about"], "News description");
    assert_eq!(
        m.methods(),
        [
            "messages.getDialogs",
            "users.getFullUser",
            "channels.getFullChannel"
        ]
    );
}
#[tokio::test]
async fn one_failed_about_lookup_does_not_drop_other_chats() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    m.fail("users.getFullUser");
    let r = run(&m, "list_chats", json!({"with_about":true}))
        .await
        .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 2);
    assert_eq!(r["results"][0]["about"], "<error fetching description>");
}
#[tokio::test]
async fn get_chats_page_uses_offset_without_marking_read() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    let r = run(&m, "get_chats", json!({"page":2,"page_size":1}))
        .await
        .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["name"], "News");
    assert_eq!(r["metadata"]["fetched"], 1);
    assert_eq!(m.sent("messages.getDialogs")[0]["limit"], 2);
    assert_eq!(m.methods(), ["messages.getDialogs"]);
}
#[tokio::test]
async fn empty_late_chat_page_never_reports_negative_fetched_count() {
    let m = Mock::new();
    m.reply("messages.getDialogs", json!({"dialogs":[]}));
    let r = run(&m, "get_chats", json!({"page":2,"page_size":100}))
        .await
        .unwrap();
    assert_eq!(r["metadata"]["fetched"], 0);
    assert_eq!(r["results"], json!([]));
}
#[tokio::test]
async fn invalid_chat_paging_is_refused_before_network() {
    let m = Mock::new();
    assert!(run(&m, "get_chats", json!({"page":0})).await.is_err());
    assert!(
        run(&m, "get_chats", json!({"page":100001,"page_size":1}))
            .await
            .is_err()
    );
    assert!(run(&m, "list_chats", json!({"limit":0})).await.is_err());
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn chat_type_filter_excludes_users_and_basic_groups_from_channel_query() {
    let m = Mock::new();
    m.reply("messages.getDialogs", dialogs());
    let r = run(&m, "list_chats", json!({"chat_type":"channel"}))
        .await
        .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["type"], "channel");
}
#[tokio::test]
async fn lookup_peer_returns_all_collectible_names_and_first_active_link() {
    let m = Mock::new();
    m.reply("users.getFullUser",json!({"users":[{"_":"user","id":"8","first_name":"Ada","last_name":null,"usernames":[{"username":"old_name","active":false},{"username":"short","active":true}]}]}));
    let r = run(&m, "lookup_peer", json!({"query":8})).await.unwrap();
    let p = &r["results"][0];
    assert_eq!(p["id"], "8");
    assert_eq!(p["bare_id"], "8");
    assert_eq!(p["name"], "Ada");
    assert_eq!(p["usernames"].as_array().unwrap().len(), 2);
    assert_eq!(p["link"], "https://t.me/short");
}
#[tokio::test]
async fn lookup_peer_accepts_link_with_post_path_and_query() {
    let m = Mock::new();
    m.set_peer(
        "Ada",
        json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
    );
    m.reply(
        "users.getFullUser",
        json!({"users":[{"_":"user","id":"8","first_name":"Ada","username":"Ada"}]}),
    );
    let r = run(
        &m,
        "lookup_peer",
        json!({"query":"https://t.me/Ada/123?single"}),
    )
    .await
    .unwrap();
    assert_eq!(r["results"][0]["link"], "https://t.me/Ada");
}
#[tokio::test]
async fn lookup_peer_never_imports_private_invite_link() {
    let m = Mock::new();
    assert!(
        run(&m, "lookup_peer", json!({"query":"https://t.me/+abc"}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn search_my_chats_honors_rule_based_folder_and_excludes_public_results() {
    let m = Mock::new();
    let bot = json!({"_":"user","id":"8","access_hash":"80","first_name":"Ada Bot","bot":true});
    m.reply("messages.getDialogFilters",json!({"filters":[{"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"My Bots","entities":[]},"bots":true,"exclude_archived":true,"exclude_muted":true,"pinned_peers":[],"include_peers":[],"exclude_peers":[]}]}));
    m.reply("contacts.search",json!({"my_results":[{"_":"peerUser","user_id":"8"}],"results":[{"_":"peerChannel","channel_id":"7"}],"users":[bot],"chats":[{"_":"channel","id":"7","access_hash":"70","title":"Ada public"}]}));
    m.reply(
        "messages.getDialogs",
        json!({"dialogs":[],"users":[],"chats":[]}),
    );
    m.reply("messages.getPeerDialogs",json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"8"},"unread_count":1,"notify_settings":{"mute_until":0}}]}));
    let r = run(
        &m,
        "search_my_chats",
        json!({"query":"Ada","folder":"My Bots"}),
    )
    .await
    .unwrap();
    assert_eq!(r["results"].as_array().unwrap().len(), 1);
    assert_eq!(r["results"][0]["type"], "bot");
    assert_eq!(r["results"][0]["folders"], json!(["My Bots"]));
    assert_eq!(m.sent("messages.getPeerDialogs").len(), 1);
}
#[tokio::test]
async fn pin_in_folder_preserves_unrelated_filter_fields() {
    let m = Mock::new();
    m.reply("messages.getDialogFilters",json!({"filters":[{"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"Work","entities":[]},"bots":true,"pinned_peers":[],"include_peers":[],"exclude_peers":[{"_":"inputPeerUser","user_id":"8","access_hash":"80"}],"exclude_muted":true}]}));
    run(&m, "pin_chat", json!({"chat":7,"folder":"Work"}))
        .await
        .unwrap();
    let filter = &m.sent("messages.updateDialogFilter")[0]["filter"];
    assert_eq!(filter["bots"], true);
    assert_eq!(filter["exclude_muted"], true);
    assert_eq!(filter["exclude_peers"][0]["user_id"], "8");
    assert_eq!(filter["pinned_peers"][0]["channel_id"], "7");
    assert!(m.sent("messages.toggleDialogPin").is_empty());
}
