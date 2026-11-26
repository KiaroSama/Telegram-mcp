use super::common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn command_menus_use_the_fallback_language_for_both_read_and_write() {
    for (tool, method, args) in [
        (
            "set_bot_commands",
            "bots.setBotCommands",
            json!({"commands":[{"command":"help","description":"Help"}]}),
        ),
        ("get_bot_commands", "bots.getBotCommands", json!({})),
    ] {
        let m = Mock::new();
        m.reply("users.getUsers", json!([{"_":"user","id":"8","bot":true}]));
        run(&m, tool, args).await.unwrap();
        assert_eq!(m.sent(method)[0]["lang_code"], "");
        assert_eq!(m.sent(method)[0]["scope"]["_"], "botCommandScopeDefault");
    }
}

fn photo(id: i64) -> Value {
    json!({"_":"photo","id":id.to_string(),"access_hash":"70","file_reference":[1,2,3]})
}

#[tokio::test]
async fn profile_replacement_collects_all_prior_pages_before_upload_and_deletes_in_batches() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":(1..=100).map(photo).collect::<Vec<_>>()}),
    );
    m.reply("photos.getUserPhotos", json!({"photos":[photo(101)]}));
    m.reply(
        "photos.deletePhotos",
        json!((1..=100).map(|id| id.to_string()).collect::<Vec<_>>()),
    );
    m.reply("photos.deletePhotos", json!(["101"]));
    let result = run(
        &m,
        "set_profile_photo",
        json!({"file_path":"new.png","replace":true}),
    )
    .await
    .unwrap();
    assert_eq!(m.sent("photos.getUserPhotos").len(), 2);
    assert_eq!(m.sent("photos.getUserPhotos")[1]["offset"], 100);
    assert_eq!(
        &m.methods()[..3],
        [
            "photos.getUserPhotos",
            "photos.getUserPhotos",
            "local.upload"
        ]
    );
    assert_eq!(m.sent("photos.deletePhotos").len(), 2);
    assert_eq!(result["old_photos_deleted"].as_array().unwrap().len(), 101);
}

#[tokio::test]
async fn deleting_a_selected_bot_history_photo_is_refused_before_any_telegram_request() {
    for (name, args) in [
        ("delete_profile_photo", json!({"bot":8,"photo_id":77})),
        (
            "set_profile_photo",
            json!({"bot":8,"replace":true,"file_path":"new.png"}),
        ),
    ] {
        let m = Mock::new();
        m.reply("photos.getUserPhotos", json!({"photos":[photo(77)]}));
        assert!(run(&m, name, args).await.is_err());
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn a_bounded_photo_history_scan_cannot_report_replacement_or_upload_after_stopping_early() {
    for profile in [false, true] {
        let m = Mock::new();
        for page in 0..100 {
            let ids = (1..=100).map(|index| 10001 - page * 100 - index);
            let response = if profile {
                json!({"photos":ids.map(photo).collect::<Vec<_>>()})
            } else {
                json!({"messages":ids.map(|id|json!({"_":"messageService","id":id,"action":{"_":"messageActionChatEditPhoto"}})).collect::<Vec<_>>()})
            };
            m.reply(
                if profile {
                    "photos.getUserPhotos"
                } else {
                    "messages.search"
                },
                response,
            );
        }
        let result = run(
            &m,
            if profile {
                "set_profile_photo"
            } else {
                "replace_chat_photo"
            },
            json!({"chat_id":7,"file_path":"new.png","replace":true}),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("bounded"));
        assert!(m.sent("local.upload").is_empty());
        assert!(m.sent("photos.deletePhotos").is_empty());
        assert!(m.sent("channels.deleteMessages").is_empty());
    }
}

#[tokio::test]
async fn profile_replacement_reports_cleanup_failure_after_the_new_photo_is_committed() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":[photo(100),photo(101)]}),
    );
    m.reply("photos.uploadProfilePhoto", json!({"photo":photo(999)}));
    m.fail("photos.deletePhotos");
    let result = run(
        &m,
        "set_profile_photo",
        json!({"file_path":"portrait.png","replace":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["photo_changed"], true);
    assert_eq!(result["old_photos_deleted"], false);
    assert!(
        result["warning"]
            .as_str()
            .unwrap()
            .contains("do not upload again")
    );
    assert_eq!(
        m.methods(),
        [
            "photos.getUserPhotos",
            "local.upload",
            "photos.uploadProfilePhoto",
            "photos.deletePhotos"
        ]
    );
}

#[tokio::test]
async fn rejected_photo_upload_leaves_the_previously_listed_photos_untouched() {
    let m = Mock::new();
    m.reply("photos.getUserPhotos", json!({"photos":[photo(100)]}));
    m.fail("photos.uploadProfilePhoto");
    assert!(
        run(
            &m,
            "set_profile_photo",
            json!({"file_path":"portrait.png","replace":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("photos.deletePhotos").is_empty());
}

#[tokio::test]
async fn deleting_an_old_profile_photo_pages_until_the_exact_id_is_found() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":(1..=100).map(photo).collect::<Vec<_>>()}),
    );
    m.reply("photos.getUserPhotos", json!({"photos":[photo(150)]}));
    run(&m, "delete_profile_photo", json!({"photo_id":"150"}))
        .await
        .unwrap();
    assert_eq!(m.sent("photos.getUserPhotos")[1]["offset"], 100);
    let removed = &m.sent("photos.deletePhotos")[0]["id"];
    assert_eq!(removed.as_array().unwrap().len(), 1);
    assert_eq!(removed[0]["id"], "150");
}

#[tokio::test]
async fn clearing_a_bot_photo_uses_the_explicit_empty_photo_without_reading_personal_photos() {
    let m = Mock::new();
    run(&m, "delete_profile_photo", json!({"bot":8}))
        .await
        .unwrap();
    assert!(m.sent("photos.getUserPhotos").is_empty());
    assert_eq!(
        m.sent("photos.updateProfilePhoto")[0]["id"]["_"],
        "inputPhotoEmpty"
    );
    assert_eq!(
        m.sent("photos.updateProfilePhoto")[0]["bot"]["user_id"],
        "8"
    );
}

#[tokio::test]
async fn device_exclusivity_reports_partial_failures_and_preserves_unrelated_call_settings() {
    let m = Mock::new();
    m.reply(
        "account.getAuthorizations",
        json!({"authorizations":[{"hash":"1"},{"hash":2,"api_id":6},{"hash":"3"}]}),
    );
    m.reply("account.changeAuthorizationSettings", json!(true));
    m.fail("account.changeAuthorizationSettings");
    m.reply("account.changeAuthorizationSettings", json!(true));
    let result = run(&m, "set_secret_chats_only_device", json!({"hash":"2"}))
        .await
        .unwrap();
    assert_eq!(result["complete"], false);
    assert_eq!(result["results"][0]["accept_secret_chats"], true);
    assert_eq!(result["results"][1]["accepted"], false);
    let changes = m.sent("account.changeAuthorizationSettings");
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["encrypted_requests_disabled"], false);
    assert_eq!(changes[1]["encrypted_requests_disabled"], true);
    assert!(
        changes
            .iter()
            .all(|request| request.get("call_requests_disabled").is_none())
    );
}

#[tokio::test]
async fn missing_or_current_authorizations_are_not_terminated_without_the_required_target_and_opt_in()
 {
    for args in [json!({"hash":999}), json!({"hash":1})] {
        let m = Mock::new();
        m.reply(
            "account.getAuthorizations",
            json!({"authorizations":[{"hash":"1","current":true}]}),
        );
        assert!(run(&m, "terminate_authorization", args).await.is_err());
        assert!(m.sent("account.resetAuthorization").is_empty());
    }
    let m = Mock::new();
    m.reply(
        "account.getAuthorizations",
        json!({"authorizations":[{"hash":"1","current":true}]}),
    );
    run(
        &m,
        "terminate_authorization",
        json!({"hash":1,"include_current":true}),
    )
    .await
    .unwrap();
    assert_eq!(m.sent("account.resetAuthorization")[0]["hash"], "1");
}

#[tokio::test]
async fn unverified_device_or_failed_enable_never_disables_others() {
    for id in [0, 2040, 6] {
        let m = Mock::new();
        m.reply(
            "account.getAuthorizations",
            json!({"authorizations":[{"hash":"1","api_id":6},{"hash":"2","api_id":id}]}),
        );
        m.reply("account.changeAuthorizationSettings", json!(false));
        let result = run(&m, "set_secret_chats_only_device", json!({"hash":"2"})).await;
        if id == 6 {
            assert_eq!(result.unwrap()["complete"], false);
            assert_eq!(m.sent("account.changeAuthorizationSettings").len(), 1);
        } else {
            assert!(result.is_err());
            assert!(m.sent("account.changeAuthorizationSettings").is_empty());
        }
    }
}
