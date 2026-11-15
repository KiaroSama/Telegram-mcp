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
