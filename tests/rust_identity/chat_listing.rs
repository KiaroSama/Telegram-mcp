use super::common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn a_basic_group_and_channel_with_the_same_bare_id_keep_separate_names_and_types() {
    let m = Mock::new();
    m.reply("messages.getDialogs",json!({"dialogs":[{"_":"dialog","peer":{"_":"peerChannel","channel_id":"42"}},{"_":"dialog","peer":{"_":"peerChat","chat_id":"42"}}],"chats":[{"_":"chat","id":"42","title":"Basic42"},{"_":"channel","id":"42","access_hash":"420","title":"Channel42","broadcast":true}]}));
    let result = run(&m, "list_chats", json!({})).await.unwrap();
    assert_eq!(result["results"][0]["name"], "Channel42");
    assert_eq!(result["results"][0]["id"], "-1000000000042");
    assert_eq!(result["results"][0]["type"], "channel");
    assert_eq!(result["results"][1]["name"], "Basic42");
    assert_eq!(result["results"][1]["id"], "-42");
}

#[tokio::test]
async fn malformed_folder_selectors_cannot_widen_search_or_change_global_pins() {
    for folder in [json!(4), json!(false), json!({}), json!([])] {
        for name in ["pin_chat", "unpin_chat", "search_my_chats"] {
            let m = Mock::new();
            assert!(
                run(&m, name, json!({"chat":7,"query":"Ada","folder":folder}))
                    .await
                    .is_err()
            );
            assert!(
                m.methods().is_empty(),
                "{name} issued a request for malformed folder {folder}"
            );
        }
    }
}

#[tokio::test]
async fn contact_fallback_batches_dialog_lookups_and_matches_response_peers() {
    let m = Mock::new();
    let users = (1..=250)
        .map(|id| json!({"_":"user","id":id.to_string(),"access_hash":"80","first_name":"Ada"}))
        .collect::<Vec<_>>();
    m.reply("messages.getDialogFilters",json!({"filters":[{"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"Chosen","entities":[]},"pinned_peers":[],"include_peers":[{"_":"inputPeerUser","user_id":"150","access_hash":"80"}],"exclude_peers":[]}]}));
    m.reply(
        "contacts.search",
        json!({"my_results":[],"users":[],"chats":[]}),
    );
    m.reply("messages.getDialogs", json!({"dialogs":[]}));
    m.reply("contacts.getContacts", json!({"users":users}));
    m.reply("messages.getPeerDialogs", json!({"dialogs":[]}));
    m.reply("messages.getPeerDialogs",json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"151"},"unread_count":99},{"_":"dialog","peer":{"_":"peerUser","user_id":"150"},"unread_count":7}]}));
    let result = run(
        &m,
        "search_my_chats",
        json!({"query":"ada","folder":"Chosen","limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["id"], "150");
    assert_eq!(result["results"][0]["unread"], 7);
    let calls = m.sent("messages.getPeerDialogs");
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| call["peers"].as_array().unwrap().len() == 100)
    );
}

#[tokio::test]
async fn contact_search_bounds_remote_work_and_discloses_an_incomplete_candidate_scan() {
    let m = Mock::new();
    m.reply("messages.getDialogFilters", json!({"filters":[]}));
    m.reply("contacts.search", json!({"my_results":[]}));
    m.reply("messages.getDialogs", json!({"dialogs":[]}));
    let users = (1..=1100)
        .map(|id| json!({"_":"user","id":id.to_string(),"access_hash":"80","first_name":"Ada"}))
        .collect::<Vec<_>>();
    m.reply("contacts.getContacts", json!({"users":users}));
    for _ in 0..10 {
        m.reply("messages.getPeerDialogs", json!({"dialogs":[]}));
    }
    let result = run(&m, "search_my_chats", json!({"query":"Ada"}))
        .await
        .unwrap();
    assert!(result["results"].as_array().unwrap().is_empty());
    assert_eq!(m.sent("messages.getPeerDialogs").len(), 10);
    assert_eq!(result["metadata"]["candidate_limit_reached"], true);
}

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
    assert_eq!(r["results"][0]["id"], r["results"][0]["chat_id"]);
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
    assert_eq!(m.resolutions(), [json!("Ada")]);
}

#[tokio::test]
async fn dialog_pagination_uses_the_last_peers_message_date_when_channel_ids_collide() {
    let m = Mock::new();
    let dialogs = (1..=100).map(|id| json!({"_":"dialog","peer":{"_":"peerChannel","channel_id":id.to_string()},"top_message":42})).collect::<Vec<_>>();
    let chats = (1..=100).map(|id| json!({"_":"channel","id":id.to_string(),"access_hash":"70","title":"Cursor fixture"})).collect::<Vec<_>>();
    m.reply("messages.getDialogs",json!({"dialogs":dialogs,"chats":chats,"messages":[{"id":42,"peer_id":{"_":"peerChannel","channel_id":"1"},"date":100},{"id":42,"peer_id":{"_":"peerChannel","channel_id":"100"},"date":200}]}));
    m.reply("messages.getDialogs", json!({"dialogs":[]}));
    run(&m, "get_chats", json!({"page":2,"page_size":100}))
        .await
        .unwrap();
    let requests = m.sent("messages.getDialogs");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["offset_peer"]["channel_id"], "100");
    assert_eq!(requests[1]["offset_id"], 42);
    assert_eq!(requests[1]["offset_date"], 200);
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
    m.reply("messages.getDialogFilters",json!({"filters":[{"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"Work","entities":[]},"bots":true,"pinned_peers":[],"include_peers":[{"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}],"exclude_peers":[{"_":"inputPeerUser","user_id":"8","access_hash":"80"}],"exclude_muted":true}]}));
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

#[tokio::test]
async fn folder_pin_refuses_shared_and_nonmember_folders() {
    for f in ["dialogFilterChatlist", "dialogFilter"] {
        let m = Mock::new();
        m.reply("messages.getDialogFilters",json!({"filters":[{"_":f,"id":2,"title":{"text":"Work"},"include_peers":[],"pinned_peers":[]}]}));
        assert!(
            run(&m, "pin_chat", json!({"chat":7,"folder":"Work"}))
                .await
                .is_err()
        );
        assert!(m.sent("messages.updateDialogFilter").is_empty());
    }
}

#[tokio::test]
async fn list_chats_skips_unmatched_dialog_before_counting() {
    let m = Mock::new();
    for (uid, unread) in [(7, 0), (8, 1)] {
        let uid = uid.to_string();
        m.reply("messages.getDialogs",json!({"dialogs":[{"peer":{"_":"peerUser","user_id":uid},"top_message":4,"unread_count":unread}],"users":[{"_":"user","id":uid,"first_name":"Reader","access_hash":"71"}]}));
    }
    let result = run(&m, "list_chats", json!({"limit":1,"unread_only":true}))
        .await
        .unwrap();
    assert_eq!(result["results"][0]["id"], "8");
    assert_eq!(m.sent("messages.getDialogs").len(), 2);
}

#[tokio::test]
async fn public_search_distinguishes_local_membership_from_global_results() {
    let m = Mock::new();
    m.reply("contacts.search",json!({"my_results":[{"_":"peerChat","chat_id":"7"}],"chats":[{"_":"chat","id":"7","title":"One"},{"_":"chat","id":"8","title":"Two"}]}));
    let r = run(&m, "search_public_chats", json!({"query":"o","limit":2}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["joined"], true);
    assert_eq!(r["results"][1]["joined"], false);
    assert_eq!(r["metadata"]["has_more"], true);
}

#[tokio::test]
async fn my_chat_search_reaches_a_name_only_on_page_two() {
    let m = Mock::new();
    m.reply("messages.getDialogFilters", json!({"filters":[]}));
    m.reply("contacts.search", json!({"my_results":[]}));
    let page = json!({"_":"messages.dialogsSlice","count":2,"dialogs":[{"peer":{"_":"peerUser","user_id":"7"},"top_message":10}],"users":[{"_":"user","id":"7","access_hash":"17","first_name":"First"}]});
    m.reply("messages.getDialogs", page);
    let dialog = json!({"peer":{"_":"peerUser","user_id":"8"},"top_message":9});
    m.reply("messages.getDialogs",json!({"_":"messages.dialogs","dialogs":[dialog],"users":[{"_":"user","id":"8","access_hash":"18","first_name":"Older"}]}));
    m.reply("messages.getPeerDialogs", json!({"dialogs":[dialog]}));
    let r = run(&m, "search_my_chats", json!({"query":"Older"}))
        .await
        .unwrap();
    assert_eq!(r["results"][0]["id"], "8");
    assert_eq!(m.sent("messages.getDialogs")[1]["offset_id"], 10);
}
