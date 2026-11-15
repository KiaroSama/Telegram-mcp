use super::common::*;
use serde_json::json;

#[tokio::test]
async fn community_membership_is_verified_before_mutation() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "rename_community",
            json!({"community":99,"title":"New"})
        )
        .await
        .is_err()
    );
    assert_eq!(m.methods(), ["communities.getJoinedCommunities"]);
}
#[tokio::test]
async fn marked_community_id_resolves_from_joined_list() {
    let m = Mock::new();
    run(
        &m,
        "rename_community",
        json!({"community":"-1000000000007","title":"New"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.sent("channels.editTitle")[0]["channel"]["channel_id"],
        "7"
    );
}
#[tokio::test]
async fn community_visibility_must_be_explicit_before_network() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "add_chat_to_community",
            json!({"community":7,"chat":9,"visibility":"guess"})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn community_link_approval_rejects_ambiguous_targets() {
    let m = Mock::new();
    assert!(
        run(&m, "approve_community_link_request", json!({"community":7}))
            .await
            .is_err()
    );
    assert!(
        run(
            &m,
            "approve_community_link_request",
            json!({"community":7,"chat":9,"all":true})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn community_who_can_add_keeps_other_default_bans() {
    let m = Mock::new();
    m.reply("communities.getJoinedCommunities",json!({"chats":[{"_":"community","id":"7","access_hash":"70","default_banned_rights":{"_":"chatBannedRights","until_date":0,"send_photos":true}}]}));
    run(
        &m,
        "set_community_who_can_add",
        json!({"community":7,"who":"admins"}),
    )
    .await
    .unwrap();
    let r = &m.sent("messages.editChatDefaultBannedRights")[0]["banned_rights"];
    assert_eq!(r["manage_linked_peers"], true);
    assert_eq!(r["send_photos"], true);
}
#[tokio::test]
async fn authorization_current_session_needs_explicit_opt_in() {
    let m = Mock::new();
    m.reply(
        "account.getAuthorizations",
        json!({"authorizations":[{"hash":"8","current":true}]}),
    );
    assert!(
        run(&m, "terminate_authorization", json!({"hash":"8"}))
            .await
            .is_err()
    );
    assert!(m.sent("account.resetAuthorization").is_empty());
}
#[tokio::test]
async fn terminate_other_authorization_uses_exact_i64_hash() {
    let m = Mock::new();
    m.reply(
        "account.getAuthorizations",
        json!({"authorizations":[{"hash":"9007199254740993","current":false}]}),
    );
    run(
        &m,
        "terminate_authorization",
        json!({"hash":"9007199254740993"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.sent("account.resetAuthorization")[0]["hash"],
        "9007199254740993"
    );
}
#[tokio::test]
async fn changing_call_authorization_does_not_change_secret_chat_settings() {
    let m = Mock::new();
    run(
        &m,
        "set_authorization_calls",
        json!({"hash":"8","accept_calls":true}),
    )
    .await
    .unwrap();
    let p = &m.sent("account.changeAuthorizationSettings")[0];
    assert_eq!(p["call_requests_disabled"], false);
    assert!(p.get("encrypted_requests_disabled").is_none());
}
#[tokio::test]
async fn profile_photo_replace_uploads_before_removing_old_photo() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":[{"_":"photo","id":"9","access_hash":"90","file_reference":[1,2]}]}),
    );
    run(
        &m,
        "set_profile_photo",
        json!({"file_path":"photo.png","replace":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.methods(),
        [
            "photos.getUserPhotos",
            "local.upload",
            "photos.uploadProfilePhoto",
            "photos.deletePhotos"
        ]
    );
    assert_eq!(m.sent("photos.deletePhotos")[0]["id"][0]["_"], "inputPhoto");
}
#[tokio::test]
async fn failed_new_profile_photo_never_deletes_old_photos() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":[{"_":"photo","id":"9","access_hash":"90","file_reference":[]}]}),
    );
    m.fail("photos.uploadProfilePhoto");
    assert!(
        run(
            &m,
            "set_profile_photo",
            json!({"file_path":"photo.png","replace":true})
        )
        .await
        .is_err()
    );
    assert!(m.sent("photos.deletePhotos").is_empty());
}
#[tokio::test]
async fn nonexistent_profile_photo_id_does_not_delete_current_photo() {
    let m = Mock::new();
    m.reply(
        "photos.getUserPhotos",
        json!({"photos":[{"_":"photo","id":"9","access_hash":"90","file_reference":[]}]}),
    );
    assert!(
        run(&m, "delete_profile_photo", json!({"photo_id":"111"}))
            .await
            .is_err()
    );
    assert!(m.sent("photos.deletePhotos").is_empty());
}
