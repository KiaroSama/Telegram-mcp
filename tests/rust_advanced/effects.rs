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
    assert_eq!(r["preview_sticker"]["document_id"], "20");
    assert_eq!(r["preview_sticker"]["asset"], "preview_sticker");
    assert!(r["preview_sticker"].get("access_hash").is_none());
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
    c["documents"][0]["mime_type"] = json!("image/webp");
    let b = Mock::new()
        .cached("advanced-test-account-b")
        .respond("messages.getAvailableEffects", c);
    let first = run(&a, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    let second = run(&b, "get_message_effect", json!({"effect_id":"100"}))
        .await
        .unwrap();
    assert!(first["preview_sticker"]["mime_type"].is_null());
    assert_eq!(second["preview_sticker"]["mime_type"], "image/webp");
    for r in [first, second] {
        assert!(r["preview_sticker"].get("file_reference").is_none());
    }
}
#[tokio::test]
async fn simultaneous_effect_reads_share_one_catalogue_fetch() {
    struct Held {
        inner: Mock,
        entered: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl Telegram for Held {
        fn cache_key(&self) -> Option<String> {
            self.inner.cache_key()
        }
        async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            self.inner.invoke(method, args).await
        }
        async fn resolve_peer(&self, value: &Value) -> Result<Value> {
            self.inner.resolve_peer(value).await
        }
    }
    let unrelated = Mock::new()
        .cached("advanced-effects-concurrent-unrelated")
        .respond("messages.getAvailableEffects", catalogue());
    run(&unrelated, "get_message_effect", json!({"effect_id":"50"}))
        .await
        .unwrap();
    let api = std::sync::Arc::new(Held {
        inner: Mock::new()
            .cached("advanced-effects-concurrent")
            .respond("messages.getAvailableEffects", catalogue()),
        entered: Default::default(),
        release: tokio::sync::Semaphore::new(0),
        calls: Default::default(),
    });
    let first = std::sync::Arc::clone(&api);
    let one = tokio::spawn(async move {
        advanced::call(
            first.as_ref(),
            "get_message_effect",
            &json!({"effect_id":"50"}),
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), api.entered.notified())
        .await
        .unwrap();
    // This cached account remains usable while another account's RPC is held.
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run(&unrelated, "get_message_effect", json!({"effect_id":"100"})),
    )
    .await
    .unwrap()
    .unwrap();
    let second = std::sync::Arc::clone(&api);
    let (started, running) = tokio::sync::oneshot::channel();
    let two = tokio::spawn(async move {
        started.send(()).unwrap();
        advanced::call(
            second.as_ref(),
            "get_message_effect",
            &json!({"effect_id":"100"}),
        )
        .await
        .unwrap()
    });
    running.await.unwrap();
    assert_eq!(api.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    api.release.add_permits(2);
    one.await.unwrap().unwrap();
    two.await.unwrap().unwrap();
    assert_eq!(api.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(unrelated.count("messages.getAvailableEffects"), 1);
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
