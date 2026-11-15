use super::common::*;
fn catalogue() -> Value {
    json!({"_":"messages.availableEffects","hash":12,"effects":[{"_":"availableEffect","id":"100","emoticon":"❤","effect_sticker_id":"20"},{"_":"availableEffect","id":"50","emoticon":"🔥","premium_required":true,"static_icon_id":"21","effect_sticker_id":"20","effect_animation_id":"22"}],"documents":[{"_":"document","id":"20","access_hash":"90","file_reference":[1],"dc_id":2},{"_":"document","id":"21","access_hash":"91","file_reference":[2],"dc_id":2},{"_":"document","id":"22","access_hash":"92","file_reference":[3],"dc_id":2}]})
}
#[tokio::test]
async fn effect_catalogue_is_cached_for_the_account_session() {
    let m = Mock::new()
        .cached("advanced-test-cached")
        .respond("messages.getAvailableEffects", catalogue());
    run(&m, "list_message_effects", json!({})).await.unwrap();
    run(&m, "list_message_effects", json!({})).await.unwrap();
    assert_eq!(m.count("messages.getAvailableEffects"), 1);
}
#[tokio::test]
async fn effect_pages_are_stable_in_id_order() {
    let m = Mock::new().respond("messages.getAvailableEffects", catalogue());
    let r = run(&m, "list_message_effects", json!({"limit":1}))
        .await
        .unwrap();
    assert_eq!(r["records"][0]["id"], "50");
    assert_eq!(r["next_offset"], 1);
}
#[tokio::test]
async fn effect_filter_honors_emoji_and_premium_flags() {
    let m = Mock::new().respond("messages.getAvailableEffects", catalogue());
    let r = run(
        &m,
        "list_message_effects",
        json!({"premium_only":true,"emoticon":"🔥"}),
    )
    .await
    .unwrap();
    assert_eq!(r["records"].as_array().unwrap().len(), 1);
    assert_eq!(r["records"][0]["id"], "50");
}
#[tokio::test]
async fn unknown_effect_uses_hash_revalidation_once() {
    let m = Mock::new()
        .cached("advanced-test-unknown")
        .respond("messages.getAvailableEffects", catalogue())
        .respond(
            "messages.getAvailableEffects",
            json!({"_":"messages.availableEffectsNotModified"}),
        );
    run(&m, "list_message_effects", json!({})).await.unwrap();
    assert!(
        run(&m, "get_message_effect", json!({"effect_id":"999"}))
            .await
            .is_err()
    );
    assert!(
        run(&m, "get_message_effect", json!({"effect_id":"999"}))
            .await
            .is_err()
    );
    let requests: Vec<_> = m
        .all()
        .into_iter()
        .filter(|(n, _)| n == "messages.getAvailableEffects")
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].1["hash"], 12);
}
#[tokio::test]
async fn cold_catalogue_miss_does_not_refetch_same_snapshot() {
    let m = Mock::new()
        .cached("advanced-test-cold")
        .respond("messages.getAvailableEffects", catalogue());
    assert!(
        run(&m, "get_message_effect", json!({"effect_id":"999"}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.getAvailableEffects"), 1);
}
#[tokio::test]
async fn effect_without_static_icon_reports_emoticon_fallback() {
    let m = Mock::new().respond("messages.getAvailableEffects", catalogue());
    let r = run(&m, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    assert_eq!(r["icon_source"], "emoticon");
    assert!(r.get("static_icon").is_none());
}
#[tokio::test]
async fn dangling_static_icon_is_not_reported_as_fallback() {
    let mut c = catalogue();
    c["effects"][0]["static_icon_id"] = json!("999");
    let m = Mock::new().respond("messages.getAvailableEffects", c);
    let r = run(&m, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    assert_eq!(r["icon_source"], "unresolved_reference");
    assert_eq!(r["static_icon"]["unresolved"], true);
}
#[tokio::test]
async fn negative_effect_page_offset_costs_no_query() {
    let m = Mock::new();
    assert!(
        run(&m, "list_message_effects", json!({"offset":-1}))
            .await
            .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn account_sessions_do_not_share_effect_file_references() {
    let a = Mock::new()
        .cached("advanced-test-account-a")
        .respond("messages.getAvailableEffects", catalogue());
    let mut c = catalogue();
    c["documents"][0]["file_reference"] = json!([99]);
    let b = Mock::new()
        .cached("advanced-test-account-b")
        .respond("messages.getAvailableEffects", c);
    let first = run(&a, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    let second = run(&b, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    assert_eq!(first["preview_sticker"]["file_reference"], json!([1]));
    assert_eq!(second["preview_sticker"]["file_reference"], json!([99]));
}
#[tokio::test]
async fn simultaneous_effect_reads_share_one_catalogue_fetch() {
    let m = Mock::new()
        .cached("advanced-effects-concurrent")
        .respond("messages.getAvailableEffects", catalogue());
    let one = json!({"effect_id":"50"});
    let two = json!({"effect_id":"100"});
    let (a, b) = tokio::join!(
        run(&m, "get_message_effect", one),
        run(&m, "get_message_effect", two)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(m.count("messages.getAvailableEffects"), 1);
}
#[tokio::test]
async fn effect_animation_metadata_names_premium_sticker_thumb_fallback() {
    let mut c = catalogue();
    c["documents"][0]["video_thumbs"] = json!([{"type":"f","w":200,"h":100,"size":4096}]);
    let m = Mock::new().respond("messages.getAvailableEffects", c);
    let r = run(&m, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    assert_eq!(r["animation_source"], "premium_effect_of_preview_sticker");
    assert_eq!(r["effect_animation"]["document_id"], "20");
    assert_eq!(r["effect_animation"]["thumb_type"], "f");
}
