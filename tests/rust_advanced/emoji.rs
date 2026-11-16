use super::common::*;
fn doc() -> Value {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        1,
        image::Rgba([17, 101, 203, 255]),
    ));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    json!({"_":"document","id":"5368324170671202286","access_hash":"456","file_reference":[1],"dc_id":2,"mime_type":"image/webp","attributes":[{"_":"documentAttributeCustomEmoji","alt":"🦆","free":true}],"thumbs":[{"_":"photoCachedSize","type":"s","w":2,"h":1,"bytes":png.into_inner()}]})
}
fn pack() -> Value {
    json!({"set":{"id":"987654321","title":"Ducks","short_name":"Ducks","emojis":true,"count":2},"packs":[{"emoticon":"Fallback","documents":["5368324170671202286"]}],"documents":[doc(),{"_":"document","id":"42","mime_type":"application/x-tgsticker","attributes":[{"_":"documentAttributeCustomEmoji","alt":"🔥","free":false}]}]})
}
#[tokio::test]
async fn installed_emoji_packs_use_emoji_specific_catalogue() {
    let m = Mock::new().respond(
        "messages.getEmojiStickers",
        json!({"sets":[pack()["set"].clone()]}),
    );
    let r = run(&m, "get_custom_emoji_packs", json!({})).await.unwrap();
    assert_eq!(r["source"], "installed");
    assert_eq!(r["packs"][0]["id"], "987654321");
    assert_eq!(r["packs"][0]["url"], "https://t.me/addemoji/Ducks");
}
#[tokio::test]
async fn featured_emoji_packs_unwrap_covered_sets() {
    let m = Mock::new().respond(
        "messages.getFeaturedEmojiStickers",
        json!({"sets":[{"set":pack()["set"].clone()}]}),
    );
    let r = run(&m, "get_custom_emoji_packs", json!({"featured":true}))
        .await
        .unwrap();
    assert_eq!(r["packs"][0]["short_name"], "Ducks");
    assert_eq!(m.count("messages.getFeaturedEmojiStickers"), 1);
}
#[tokio::test]
async fn emoji_pack_search_uses_search_request() {
    let m = Mock::new().respond(
        "messages.searchEmojiStickerSets",
        json!({"sets":[pack()["set"].clone()]}),
    );
    run(&m, "get_custom_emoji_packs", json!({"query":" ducks "}))
        .await
        .unwrap();
    assert_eq!(m.request("messages.searchEmojiStickerSets")["q"], "ducks");
}
#[tokio::test]
async fn emoji_pack_metadata_has_lossless_ids_and_accurate_formats() {
    let m = Mock::new().respond("messages.getStickerSet", pack());
    let r = run(
        &m,
        "get_custom_emoji_pack",
        json!({"pack":"https://t.me/addemoji/Ducks"}),
    )
    .await
    .unwrap();
    assert_eq!(r["emojis"][0]["document_id"], "5368324170671202286");
    assert_eq!(r["emojis"][0]["emoji"], "🦆");
    assert_eq!(r["emojis"][0]["free"], true);
    assert_eq!(r["emojis"][1]["format"], "animated");
    assert!(r.get("content").is_none());
    assert!(!r.to_string().contains("access_hash"));
}
#[tokio::test]
async fn emoji_pack_paging_has_real_continuation() {
    let m = Mock::new().respond("messages.getStickerSet", pack());
    let r = run(
        &m,
        "get_custom_emoji_pack",
        json!({"pack":"Ducks","limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(r["emojis"].as_array().unwrap().len(), 1);
    assert_eq!(r["next_offset"], 1);
}
#[tokio::test]
async fn emoji_pack_thumbnail_is_real_image_data_not_fallback_glyph() {
    use base64::Engine;
    let m = Mock::new().respond("messages.getStickerSet", pack());
    let r = run(
        &m,
        "get_custom_emoji_pack",
        json!({"pack":"Ducks","limit":1,"include_thumbnails":true}),
    )
    .await
    .unwrap();
    assert_eq!(r["emojis"][0]["preview_available"], true);
    assert_eq!(r["content"][0]["type"], "image");
    let data = base64::engine::general_purpose::STANDARD
        .decode(r["content"][0]["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&data).unwrap().to_rgba8();
    assert_eq!(image.get_pixel(0, 0).0, [17, 101, 203, 255]);
}
#[tokio::test]
async fn preview_custom_emoji_alias_fetches_one_exact_document() {
    let m = Mock::new().respond("messages.getCustomEmojiDocuments", json!([doc()]));
    let r = run(
        &m,
        "preview_custom_emoji",
        json!({"document_id":"5368324170671202286"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("messages.getCustomEmojiDocuments")["document_id"],
        json!(["5368324170671202286"])
    );
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["thumbnail_used"], true);
}
#[tokio::test]
async fn invalid_custom_emoji_document_id_is_refused_before_rpc() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "preview_custom_emoji",
            json!({"document_id":"not-a-number"})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn custom_emoji_pack_rejects_lookalike_hosts() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "get_custom_emoji_pack",
            json!({"pack":"https://t.me.evil.test/addemoji/Ducks"})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn vote_poll_alias_translates_indices_to_existing_wire_path() {
    let m = with_poll(poll());
    run(
        &m,
        "vote_poll",
        json!({"chat_id":7,"message_id":20,"option_indices":[1]}),
    )
    .await
    .unwrap();
    assert_eq!(m.request("messages.sendVote")["options"], json!([[241, 2]]));
}
#[tokio::test]
async fn vote_poll_alias_keeps_upstream_strict_nonempty_unique_indices() {
    for indices in [
        json!([]),
        json!([1, 1]),
        json!([true]),
        json!([1.0]),
        json!(["1"]),
    ] {
        let m = Mock::new();
        assert!(
            run(
                &m,
                "vote_poll",
                json!({"chat_id":7,"message_id":20,"option_indices":indices})
            )
            .await
            .is_err()
        );
        assert!(m.all().is_empty());
    }
}
