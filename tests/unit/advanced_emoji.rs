use super::*;
#[test]
fn plain_emoji_pack_name_is_preserved() {
    assert_eq!(short_name("  Duck_Pack_2026  ").unwrap(), "Duck_Pack_2026");
}
#[test]
fn official_emoji_and_sticker_urls_are_supported() {
    for raw in [
        "https://t.me/addemoji/Ducks",
        "t.me/addemoji/Ducks",
        "http://telegram.me/addstickers/Ducks",
        "tg://addemoji?set=Ducks",
        "tg://addstickers?set=Ducks",
    ] {
        assert_eq!(short_name(raw).unwrap(), "Ducks");
    }
}
#[test]
fn emoji_pack_url_rejects_untrusted_hosts_and_credentials() {
    for raw in [
        "https://t.me.evil.test/addemoji/Ducks",
        "https://user@t.me/addemoji/Ducks",
        "https://example.test/addemoji/Ducks",
        "tg://addemoji?set=One&set=Two",
        "https://t.me/addemoji/Ducks/extra",
        "",
    ] {
        assert!(short_name(raw).is_err(), "Accepted {raw}");
    }
}

#[test]
fn emoji_pack_metadata_distinguishes_sticker_urls_and_preserves_installation_date() {
    let result=metadata(&json!({"id":"42","title":"Ducks","short_name":"Ducks","count":3,"installed_date":123,"official":true})).unwrap();
    assert_eq!(result["url"], "https://t.me/addstickers/Ducks");
    assert_eq!(result["installed_date"], 123);
    assert_eq!(result["official"], true);
    assert_eq!(result["is_emoji"], false);
    assert!(metadata(&json!({"id":"43","title":"No slug"})).unwrap()["url"].is_null());
}

#[test]
fn emoji_attributes_override_pack_fallbacks_and_identify_animation_and_video() {
    let fallback = BTreeMap::from([(42, "Pack fallback".into())]);
    let sticker = emoji_record(
        &json!({"id":42,"attributes":[{"_":"documentAttributeSticker","alt":"🦆"},
        {"_":"documentAttributeVideo"},{"_":"unrelatedAttribute"}]}),
        &fallback,
    )
    .unwrap();
    assert_eq!(sticker["emoji"], "🦆");
    assert_eq!(sticker["format"], "video");
    let animated = emoji_record(
        &json!({"id":42,"attributes":[{"_":"documentAttributeAnimated"}]}),
        &fallback,
    )
    .unwrap();
    assert_eq!(animated["format"], "animated");
    assert_eq!(animated["emoji"], "Pack fallback");
}
