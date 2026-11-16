use super::*;
use base64::Engine;
use std::{collections::VecDeque, sync::Mutex};

struct Api {
    key: Option<String>,
    catalogues: Mutex<VecDeque<Value>>,
    files: Mutex<VecDeque<std::result::Result<Vec<u8>, &'static str>>>,
    requests: Mutex<Vec<Value>>,
    downloads: Mutex<Vec<(Value, u64)>>,
}
impl Api {
    fn new(catalogues: Vec<Value>, files: Vec<std::result::Result<Vec<u8>, &'static str>>) -> Self {
        Self {
            key: None,
            catalogues: Mutex::new(catalogues.into()),
            files: Mutex::new(files.into()),
            requests: Mutex::new(vec![]),
            downloads: Mutex::new(vec![]),
        }
    }
}
#[async_trait::async_trait]
impl Telegram for Api {
    fn cache_key(&self) -> Option<String> {
        self.key.clone()
    }
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        assert_eq!(method, "messages.getAvailableEffects");
        crate::tl::Schema::global().encode_call(method, &params)?;
        self.requests.lock().unwrap().push(params);
        self.catalogues
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("Unexpected catalogue fetch"))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        anyhow::bail!("No peer lookup expected")
    }
    async fn download_memory(&self, location: Value, max: u64) -> Result<Vec<u8>> {
        let mut wire = location.clone();
        wire.as_object_mut().unwrap().remove("_dc_id");
        crate::tl::Schema::global().encode_object(&wire)?;
        self.downloads.lock().unwrap().push((location, max));
        self.files
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected download")
            .map_err(|message| anyhow::anyhow!(message))
    }
}
fn data() -> Value {
    json!({"_":"messages.availableEffects","hash":"12","effects":[{"_":"availableEffect","id":"50","emoticon":"🔥","static_icon_id":"21","effect_sticker_id":"20","effect_animation_id":"22"}],
        "documents":[{"_":"document","id":"20","access_hash":"90","file_reference":[1],"dc_id":2},
            {"_":"document","id":"21","access_hash":"91","file_reference":[2],"dc_id":2},
            {"_":"document","id":"22","access_hash":"92","file_reference":[3],"dc_id":4}]})
}
fn png() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([19, 83, 211, 255]),
    ));
    let mut out = std::io::Cursor::new(vec![]);
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn effect_locations_distinguish_full_assets_and_premium_sticker_thumbnail() {
    let mut catalogue = data();
    let effect = &catalogue["effects"][0];
    for (asset, id, reference, dc) in [
        ("icon", "21", json!([2]), 2),
        ("sticker", "20", json!([1]), 2),
        ("animation", "22", json!([3]), 4),
    ] {
        let located = location(&catalogue, effect, asset).unwrap();
        assert_eq!(located["id"], id);
        assert_eq!(located["file_reference"], reference);
        assert_eq!(located["_dc_id"], dc);
        assert_eq!(located["thumb_size"], "");
    }
    catalogue["effects"][0]
        .as_object_mut()
        .unwrap()
        .remove("effect_animation_id");
    catalogue["documents"][0]["video_thumbs"] =
        json!([{"type":"m"},{"type":"f","w":200,"h":100,"size":4096}]);
    let premium = location(&catalogue, &catalogue["effects"][0], "animation").unwrap();
    assert_eq!(premium["id"], "20");
    assert_eq!(premium["thumb_size"], "f");
}

#[test]
fn missing_effect_assets_remain_unresolved_without_downloadable_fallbacks() {
    let mut catalogue = data();
    catalogue["effects"][0]
        .as_object_mut()
        .unwrap()
        .remove("effect_animation_id");
    assert!(
        location(&catalogue, &catalogue["effects"][0], "animation")
            .unwrap_err()
            .to_string()
            .contains("no animation")
    );
    catalogue["effects"][0]
        .as_object_mut()
        .unwrap()
        .remove("static_icon_id");
    assert!(
        location(&catalogue, &catalogue["effects"][0], "icon")
            .unwrap_err()
            .to_string()
            .contains("no requested asset")
    );
    catalogue["effects"][0]["effect_sticker_id"] = json!("404");
    assert_eq!(
        metadata(&catalogue, &catalogue["effects"][0])["animation_source"],
        "unresolved_reference"
    );
    catalogue["effects"][0]["effect_animation_id"] = json!("405");
    let record = metadata(&catalogue, &catalogue["effects"][0]);
    assert_eq!(record["animation_source"], "unresolved_reference");
    assert_eq!(record["effect_animation"]["unresolved"], true);
}

#[tokio::test]
async fn effect_icon_preview_contains_the_actual_downloaded_pixels() {
    let api = Api::new(vec![data()], vec![Ok(png())]);
    let output = run(
        &api,
        "get_message_effect",
        &json!({"effect_id":"50","asset":"icon","count":8,"max_bytes":4096,"max_dimension":64}),
    )
    .await
    .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(output["content"][0]["data"].as_str().unwrap())
        .unwrap();
    let pixels = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(pixels.get_pixel(0, 0).0, [19, 83, 211, 255]);
    assert_eq!(output["asset"], "icon");
    assert_eq!(output["effect"]["effect_id"], "50");
    assert_eq!(api.downloads.lock().unwrap()[0].1, 4096);
    assert_eq!(api.downloads.lock().unwrap()[0].0["id"], "21");
}

#[tokio::test]
async fn expired_effect_references_refresh_once_with_hash_zero_and_use_the_new_reference() {
    for error in [
        "FILE_REFERENCE_EXPIRED",
        "FILE_REFERENCE_EMPTY",
        "FILE_REFERENCE_INVALID",
    ] {
        let mut fresh = data();
        fresh["documents"][0]["file_reference"] = json!([99]);
        let api = Api::new(vec![data(), fresh], vec![Err(error), Ok(png())]);
        let output = run(
            &api,
            "get_message_effect",
            &json!({"effect_id":50,"asset":"sticker","count":2}),
        )
        .await
        .unwrap();
        let reads = api.downloads.lock().unwrap();
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[1].0["file_reference"], json!([99]));
        assert_eq!(api.requests.lock().unwrap()[1]["hash"], 0);
        assert_eq!(output["asset"], "sticker");
        assert_eq!(
            output["effect"]["preview_sticker"]["file_reference"],
            json!([99])
        );
    }
}

#[tokio::test]
async fn effect_download_failures_never_retry_unrelated_errors_or_missing_refreshed_effects() {
    let api = Api::new(vec![data()], vec![Err("NETWORK_BROKEN")]);
    assert!(
        run(
            &api,
            "get_message_effect",
            &json!({"effect_id":50,"asset":"animation"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("NETWORK_BROKEN")
    );
    assert_eq!(api.requests.lock().unwrap().len(), 1);
    assert_eq!(api.downloads.lock().unwrap().len(), 1);
    let mut removed = data();
    removed["effects"] = json!([]);
    let api = Api::new(vec![data(), removed], vec![Err("FILE_REFERENCE_EXPIRED")]);
    assert!(
        run(
            &api,
            "get_message_effect",
            &json!({"effect_id":50,"asset":"animation"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("removed during")
    );
    assert_eq!(api.downloads.lock().unwrap().len(), 1);
    let api = Api::new(
        vec![data(), data()],
        vec![Err("FILE_REFERENCE_EXPIRED"), Err("FILE_REFERENCE_EXPIRED")],
    );
    assert!(
        run(
            &api,
            "get_message_effect",
            &json!({"effect_id":50,"asset":"animation"})
        )
        .await
        .is_err()
    );
    assert_eq!(api.downloads.lock().unwrap().len(), 2);
    assert_eq!(api.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_effect_preview_limits_are_rejected_before_catalogue_or_download() {
    for patch in [
        json!({"asset":"glyph"}),
        json!({"max_bytes":0}),
        json!({"max_bytes":52428801}),
        json!({"max_dimension":63}),
        json!({"max_dimension":2049}),
        json!({"count":0}),
    ] {
        let api = Api::new(vec![], vec![]);
        let mut input = json!({"effect_id":50});
        input
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(run(&api, "get_message_effect", &input).await.is_err());
        assert!(api.requests.lock().unwrap().is_empty());
        assert!(api.downloads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn effect_catalogue_cache_bounds_age_and_negative_misses_without_replacing_valid_data_on_errors()
 {
    let mut api = Api::new(vec![data()], vec![]);
    api.key = Some(format!("effect-boundary-{}", rand::random::<u64>()));
    catalogue(&api, &json!({}), None, false).await.unwrap();
    let key = api.key.as_ref().unwrap();
    {
        let entry = CACHE.get().unwrap().lock().await.get(key).unwrap().clone();
        let mut cached = entry.lock().await;
        let saved = cached.as_mut().unwrap();
        saved.checked = Instant::now() - Duration::from_secs(3601);
        saved.unknown = (0..1024).collect();
    }
    api.catalogues
        .lock()
        .unwrap()
        .push_back(json!({"_":"messages.availableEffectsNotModified"}));
    catalogue(&api, &json!({}), Some(2048), false)
        .await
        .unwrap();
    assert_eq!(api.requests.lock().unwrap()[1]["hash"], "12");
    {
        let entry = CACHE.get().unwrap().lock().await.get(key).unwrap().clone();
        let cached = entry.lock().await;
        let saved = cached.as_ref().unwrap();
        assert_eq!(saved.unknown.len(), 1);
        assert!(saved.unknown.contains(&2048));
    }
    api.catalogues
        .lock()
        .unwrap()
        .push_back(json!({"effects":[],"documents":null}));
    assert!(catalogue(&api, &json!({}), None, true).await.is_err());
    assert_eq!(
        catalogue(&api, &json!({}), Some(50), false).await.unwrap()["effects"][0]["id"],
        "50"
    );
    {
        let mut cache = CACHE.get().unwrap().lock().await;
        while cache.len() < 64 {
            let k = format!("bounded-fixture-{}", cache.len());
            cache.insert(
                k,
                Arc::new(tokio::sync::Mutex::new(Some(Catalogue {
                    data: data(),
                    checked: Instant::now(),
                    unknown: HashSet::new(),
                }))),
            );
        }
    }
    let mut next = Api::new(vec![data()], vec![]);
    next.key = Some(format!("effect-new-{}", rand::random::<u64>()));
    catalogue(&next, &json!({}), None, false).await.unwrap();
    let cache = CACHE.get().unwrap().lock().await;
    assert!(cache.contains_key(next.key.as_ref().unwrap()));
    assert!(cache.len() <= 64);
    assert!(!cache.keys().any(|key| key.starts_with("bounded-fixture-")));
    drop(cache);
    let mut cold = Api::new(
        vec![json!({"_":"messages.availableEffectsNotModified"})],
        vec![],
    );
    cold.key = Some(format!("effect-cold-{}", rand::random::<u64>()));
    assert!(
        catalogue(&cold, &json!({}), None, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("without a cached catalogue")
    );
}
