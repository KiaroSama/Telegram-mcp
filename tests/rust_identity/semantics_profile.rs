use super::common::*;
use serde_json::{Value, json};

fn photo(id: i64) -> Value {
    json!({"_":"photo","id":id.to_string(),"access_hash":"70","file_reference":[1,2,3]})
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
        json!({"authorizations":[{"hash":"1"},{"hash":2},{"hash":"3"}]}),
    );
    m.reply("account.changeAuthorizationSettings", json!(true));
    m.fail("account.changeAuthorizationSettings");
    m.reply("account.changeAuthorizationSettings", json!(true));
    let result = run(&m, "set_secret_chats_only_device", json!({"hash":"2"}))
        .await
        .unwrap();
    assert_eq!(result["complete"], false);
    assert_eq!(result["results"][1]["accept_secret_chats"], true);
    assert_eq!(result["results"][1]["accepted"], false);
    let changes = m.sent("account.changeAuthorizationSettings");
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["encrypted_requests_disabled"], true);
    assert_eq!(changes[1]["encrypted_requests_disabled"], false);
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
