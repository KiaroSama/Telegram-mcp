use super::common::*;
use serde_json::json;

#[tokio::test]
async fn rotating_primary_invite_returns_the_replacement_without_creating_another_link() {
    let m = Mock::new();
    m.reply(
        "messages.getExportedChatInvites",
        json!({"invites":[{"_":"chatInviteExported","permanent":true,"link":"https://t.me/+old"}]}),
    );
    let replacement =
        json!({"_":"chatInviteExported","permanent":true,"link":"https://t.me/+replacement"});
    m.reply("messages.editExportedChatInvite",json!({"_":"messages.exportedChatInviteReplaced","invite":{"_":"chatInviteExported","revoked":true,"link":"https://t.me/+old"},"new_invite":replacement,"users":[]}));
    let result = run(&m, "export_chat_invite", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(result, replacement);
    assert!(m.sent("messages.exportChatInvite").is_empty());
}

#[tokio::test]
async fn invite_approval_and_usage_limit_are_incompatible() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "create_invite_link",
            json!({"chat_id":7,"requires_approval":true,"usage_limit":5})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn edit_invite_omits_untouched_constraints() {
    let m = Mock::new();
    run(
        &m,
        "edit_invite_link",
        json!({"chat_id":7,"link":"https://t.me/+abc","title":"New"}),
    )
    .await
    .unwrap();
    let p = &m.sent("messages.editExportedChatInvite")[0];
    assert_eq!(p["title"], "New");
    assert!(p.get("expire_date").is_none());
    assert!(p.get("usage_limit").is_none());
    assert!(p.get("request_needed").is_none());
}
#[tokio::test]
async fn edit_invite_zero_explicitly_clears_expiry_and_usage() {
    let m = Mock::new();
    run(&m,"edit_invite_link",json!({"chat_id":7,"link":"https://t.me/+abc","expire_seconds":0,"usage_limit":0,"requires_approval":false})).await.unwrap();
    let p = &m.sent("messages.editExportedChatInvite")[0];
    assert_eq!(p["expire_date"], 0);
    assert_eq!(p["usage_limit"], 0);
    assert_eq!(p["request_needed"], false);
}
#[tokio::test]
async fn create_additional_invite_does_not_rotate_primary_link() {
    let m = Mock::new();
    run(
        &m,
        "create_invite_link",
        json!({"chat_id":7,"title":"Guests"}),
    )
    .await
    .unwrap();
    assert_eq!(m.methods(), ["messages.exportChatInvite"]);
    assert!(
        m.sent("messages.exportChatInvite")[0]
            .get("legacy_revoke_permanent")
            .is_none()
    );
}
#[tokio::test]
async fn private_invite_link_is_imported_by_hash() {
    let m = Mock::new();
    run(
        &m,
        "join_chat_by_link",
        json!({"link":"https://t.me/+abc_DEF-123"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.sent("messages.importChatInvite")[0]["hash"],
        "abc_DEF-123"
    );
}
#[tokio::test]
async fn malformed_invite_hash_never_reaches_telegram() {
    let m = Mock::new();
    assert!(
        run(&m, "import_chat_invite", json!({"hash":"abc?evil"}))
            .await
            .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn revoked_link_delete_failure_reports_revocation_succeeded() {
    let m = Mock::new();
    m.fail("messages.deleteExportedChatInvite");
    let r = run(
        &m,
        "revoke_invite_link",
        json!({"chat_id":7,"link":"https://t.me/+abc","delete":true}),
    )
    .await
    .unwrap();
    assert_eq!(r["revoked"], true);
    assert_eq!(r["deleted"], false);
}
#[tokio::test]
async fn qr_is_a_real_mcp_png_image_with_quiet_border_without_network() {
    let m = Mock::new();
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = telegram_mcp::state::State::at_with_installation(
        temp.path().join("state"),
        vec![files],
        temp.path().to_owned(),
    )
    .unwrap();
    let r = telegram_mcp::tools::identity::invite_link_qr_in(
        &state,
        &json!({"link":"https://t.me/+abc"}),
    )
    .unwrap();
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["content"][0]["mimeType"], "image/png");
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(r["content"][0]["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&bytes).unwrap().into_luma8();
    let published = std::path::Path::new(
        r["path"]
            .as_str()
            .expect("QR must return its published path"),
    );
    assert!(published.starts_with(state.default_downloads().unwrap()));
    assert_eq!(std::fs::read(published).unwrap(), bytes);
    assert!(image.width() >= 256);
    assert_eq!(image.width(), image.height());
    assert_eq!(image.get_pixel(0, 0).0, [255]);
    assert!(image.pixels().any(|pixel| pixel.0 == [0]));
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn channel_username_normalizes_telegram_link() {
    let m = Mock::new();
    run(
        &m,
        "check_channel_username",
        json!({"chat_id":7,"username":"https://t.me/ops_room"}),
    )
    .await
    .unwrap();
    assert_eq!(m.sent("channels.checkUsername")[0]["username"], "ops_room");
}
#[tokio::test]
async fn username_removal_requires_explicit_empty_string_and_skips_check() {
    let m = Mock::new();
    assert!(
        run(&m, "set_channel_username", json!({"chat_id":7}))
            .await
            .is_err()
    );
    run(
        &m,
        "set_channel_username",
        json!({"chat_id":7,"username":""}),
    )
    .await
    .unwrap();
    assert_eq!(m.methods(), ["channels.updateUsername"]);
    assert_eq!(m.sent("channels.updateUsername")[0]["username"], "");
}
#[tokio::test]
async fn invalid_username_does_not_trigger_resolution_or_requests() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "check_channel_username",
            json!({"chat_id":7,"username":"3badname"})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn reorder_preserves_short_collectible_usernames_and_order() {
    let m = Mock::new();
    run(
        &m,
        "reorder_channel_usernames",
        json!({"chat_id":7,"order":["b","@a"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.sent("channels.reorderUsernames")[0]["order"],
        json!(["b", "a"])
    );
}
#[tokio::test]
async fn reorder_empty_or_duplicate_usernames_refuses_before_network() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "reorder_channel_usernames",
            json!({"chat_id":7,"order":[]})
        )
        .await
        .is_err()
    );
    assert!(
        run(
            &m,
            "reorder_channel_usernames",
            json!({"chat_id":7,"order":["name","@name"]})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn signatures_disabled_also_disables_profile_signatures() {
    let m = Mock::new();
    run(
        &m,
        "set_signatures",
        json!({"chat_id":7,"enabled":false,"show_profiles":true}),
    )
    .await
    .unwrap();
    let p = &m.sent("channels.toggleSignatures")[0];
    assert_eq!(p["signatures_enabled"], false);
    assert_eq!(p["profiles_enabled"], false);
}
#[tokio::test]
async fn invalid_geo_location_is_refused_before_network() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_channel_location",
            json!({"chat_id":7,"latitude":91.0,"longitude":0.0,"address":"Place"})
        )
        .await
        .is_err()
    );
    assert!(m.methods().is_empty());
}
#[tokio::test]
async fn channel_emoji_status_preserves_large_integer_string() {
    let m = Mock::new();
    run(
        &m,
        "set_channel_emoji_status",
        json!({"chat_id":7,"custom_emoji_id":"9007199254740993"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.sent("channels.updateEmojiStatus")[0]["emoji_status"]["document_id"],
        "9007199254740993"
    );
}
