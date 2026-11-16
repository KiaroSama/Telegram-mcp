use image::{DynamicImage, Rgba, RgbaImage};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use telegram_mcp::emoji::{compose, vision};

struct EmojiWire;
#[async_trait::async_trait]
impl telegram_mcp::backend::Telegram for EmojiWire {
    async fn invoke(
        &self,
        method: &str,
        request: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        assert_eq!(method, "messages.getCustomEmojiDocuments");
        assert_eq!(
            request["document_id"],
            json!(["9007199254740993", "9007199254740994"])
        );
        Ok(json!([
            {"_":"document","id":"9007199254740993","access_hash":"3","file_reference":[],"mime_type":"image/webp","attributes":[{"_":"documentAttributeCustomEmoji","alt":"🔴"}]},
            {"_":"document","id":"9007199254740994","access_hash":"4","file_reference":[],"mime_type":"image/webp","attributes":[{"_":"documentAttributeCustomEmoji","alt":"🔵"}]}
        ]))
    }
    async fn resolve_peer(&self, _: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Emoji previews must not resolve an unrelated chat")
    }
    async fn download_memory(
        &self,
        location: serde_json::Value,
        _: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let colour = if location["id"] == "9007199254740993" {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        };
        Ok(telegram_mcp::tools::visual::images::encode_image(
            &DynamicImage::ImageRgba8(RgbaImage::from_pixel(12, 12, Rgba(colour))),
            "webp",
            64,
            90,
            false,
        )?
        .0)
    }
}

#[tokio::test]
async fn native_emoji_preview_keeps_individual_document_pixels_and_metadata() {
    let result = telegram_mcp::tools::messaging::call(
        &EmojiWire, "get_custom_emoji",
        &json!({"document_ids":["9007199254740993","9007199254740994"],"count":3,"max_dimension":64})
    ).await.unwrap().unwrap();
    assert_eq!(result["content"].as_array().unwrap().len(), 2);
    assert_eq!(result["metadata"]["emoji"][0]["placeholder"], "🔴");
    let reply = telegram_mcp::emoji::studio::ToolReply::parse(json!({
        "content":result["content"], "structuredContent":result
    }))
    .unwrap();
    let (frames, metadata) = telegram_mcp::emoji::studio::group_frames(&reply).unwrap();
    assert_eq!(metadata.len(), 2);
    for (id, expected) in [
        ("9007199254740993", [255, 0, 0, 255]),
        ("9007199254740994", [0, 0, 255, 255]),
    ] {
        assert_eq!(
            frames[id].len(),
            1,
            "A static image is one real frame regardless of requested samples"
        );
        let image = telegram_mcp::tools::visual::images::decode_image(&frames[id][0].0)
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (12, 12));
        assert_eq!(image.get_pixel(5, 5).0, expected);
        assert_eq!(frames[id][0].1["blank"], false);
    }
}

fn disc(colour: [u8; 4], radius: f64, shift: (f64, f64)) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(128, 128, |x, y| {
        let (dx, dy) = (f64::from(x) - 64.0 - shift.0, f64::from(y) - 64.0 - shift.1);
        if dx * dx + dy * dy <= (128.0 * radius).powi(2) {
            Rgba(colour)
        } else {
            Rgba([0, 0, 0, 0])
        }
    }))
}
fn square() -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(128, 128, |x, y| {
        if (20..108).contains(&x) && (20..108).contains(&y) {
            Rgba([220, 30, 30, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    }))
}
fn red() -> DynamicImage {
    disc([220, 30, 30, 255], 0.4, (0.0, 0.0))
}
fn distance(left: &DynamicImage, right: &DynamicImage) -> f64 {
    vision::distance(
        &vision::fingerprint(left).unwrap(),
        &vision::fingerprint(right).unwrap(),
    )
    .total
}

#[test]
fn visual_fingerprint_identical_image_has_zero_distance() {
    assert_eq!(distance(&red(), &red()), 0.0);
}
#[test]
fn visual_fingerprint_distance_is_symmetric() {
    let blue = disc([30, 30, 220, 255], 0.4, (0.0, 0.0));
    assert!((distance(&red(), &blue) - distance(&blue, &red())).abs() < 1e-12);
}
#[test]
fn visual_fingerprint_colour_has_real_identity_weight() {
    assert!(distance(&red(), &disc([30, 30, 220, 255], 0.4, (0.0, 0.0))) > 0.15);
}
#[test]
fn visual_fingerprint_silhouette_distinguishes_equal_colour() {
    assert!(distance(&red(), &square()) > 0.05);
}
#[test]
fn visual_fingerprint_normalises_scale_without_stretching() {
    assert!(
        distance(
            &disc([220, 30, 30, 255], 0.45, (0.0, 0.0)),
            &disc([220, 30, 30, 255], 0.18, (0.0, 0.0))
        ) < 0.12
    );
}
#[test]
fn visual_fingerprint_normalises_off_centre_padding() {
    assert!(distance(&red(), &disc([220, 30, 30, 255], 0.4, (22.0, -17.0))) < 0.12);
}
#[test]
fn visual_fingerprint_blank_is_finite_and_deterministic() {
    let blank = DynamicImage::new_rgba8(64, 64);
    assert_eq!(distance(&blank, &blank), 0.0);
    assert!((0.0..=1.0).contains(&distance(&blank, &red())));
}
#[test]
fn visual_fingerprint_has_fixed_dimensions_and_explainable_parts() {
    let fp = vision::fingerprint(&red()).unwrap();
    assert_eq!(
        (fp.silhouette.len(), fp.colour.len(), fp.structure.len()),
        (144, 108, 144)
    );
    let parts = vision::distance(&fp, &vision::fingerprint(&square()).unwrap());
    for score in [parts.total, parts.silhouette, parts.colour, parts.structure] {
        assert!((0.0..=1.0).contains(&score));
    }
}
#[test]
fn visual_fingerprint_json_round_trip_preserves_distance() {
    let fp = vision::fingerprint(&red()).unwrap();
    let restored = serde_json::from_str(&serde_json::to_string(&fp).unwrap()).unwrap();
    // Decimal cache serialization can change a binary float's least significant
    // bit; that error must stay far below one representable image-channel step.
    assert!(vision::distance(&fp, &restored).total <= f64::EPSILON);
    let blue = vision::fingerprint(&disc([30, 30, 220, 255], 0.4, (0.0, 0.0))).unwrap();
    assert!(vision::distance(&fp, &restored).total < vision::distance(&fp, &blue).total);
}
#[test]
fn visual_ranking_never_uses_the_fallback_glyph_as_identity() {
    let query = vision::fingerprint(&red()).unwrap();
    let mut correct = serde_json::to_value(&query).unwrap();
    correct["glyph"] = "💳".into();
    let mut wrong = serde_json::to_value(
        vision::fingerprint(&disc([30, 30, 220, 255], 0.4, (0.0, 0.0))).unwrap(),
    )
    .unwrap();
    wrong["glyph"] = "🔴".into();
    let index = BTreeMap::from([("correct".into(), correct), ("wrong".into(), wrong)]);
    assert_eq!(
        vision::rank(&query, &index, 2, &BTreeSet::new())[0].1,
        "correct"
    );
}
#[test]
fn visual_ranking_excludes_query_and_keeps_explanations_finite() {
    let fp = vision::fingerprint(&red()).unwrap();
    let index = BTreeMap::from([
        ("self".into(), serde_json::to_value(&fp).unwrap()),
        ("bad".into(), json!({})),
    ]);
    let rank = vision::rank(&fp, &index, 8, &BTreeSet::from(["self".into()]));
    assert_eq!(rank, vec![(1.0, "bad".into())]);
}

#[test]
fn insert_places_emoji_at_requested_line_end() {
    let (text, entities) =
        compose::insert_emoji("first\nsecond\nthird", &[], 2, "✅", "123", "end").unwrap();
    assert_eq!(text, "first\nsecond✅\nthird");
    assert_eq!(
        entities[0],
        json!({"type":"custom_emoji","offset":12,"length":1,"custom_emoji_id":"123"})
    );
}
#[test]
fn insert_at_line_start_counts_non_bmp_utf16_units() {
    let (text, entities) =
        compose::insert_emoji("alpha\nbeta", &[], 2, "🔥", "9", "start").unwrap();
    assert_eq!(text, "alpha\n🔥beta");
    assert_eq!(entities[0]["offset"], 6);
    assert_eq!(entities[0]["length"], 2);
}
#[test]
fn insert_leaves_earlier_entities_unchanged() {
    let entity = json!({"type":"bold","offset":0,"length":5});
    let (_, entities) = compose::insert_emoji(
        "alpha\nbeta",
        std::slice::from_ref(&entity),
        2,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    assert!(entities.contains(&entity));
}
#[test]
fn insert_shifts_later_entities_without_growing_them() {
    let (_, entities) = compose::insert_emoji(
        "a\nb\ncc",
        &[json!({"type":"bold","offset":4,"length":2})],
        1,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    let bold = entities.iter().find(|e| e["type"] == "bold").unwrap();
    assert_eq!(bold["offset"], 6);
    assert_eq!(bold["length"], 2);
}
#[test]
fn insert_grows_a_straddling_entity_instead_of_sliding_it() {
    let (_, entities) = compose::insert_emoji(
        "alpha\nbeta",
        &[json!({"type":"blockquote","offset":0,"length":8})],
        2,
        "🔥",
        "9",
        "start",
    )
    .unwrap();
    let quote = entities.iter().find(|e| e["type"] == "blockquote").unwrap();
    assert_eq!(quote["offset"], 0);
    assert_eq!(quote["length"], 10);
}
#[test]
fn insert_blockquote_takes_in_an_exact_edge_glyph() {
    let (_, entities) = compose::insert_emoji(
        "alpha\nbeta",
        &[json!({"_":"messageEntityBlockquote","offset":0,"length":10})],
        2,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    assert_eq!(entities[0]["length"], 12);
}
#[test]
fn insert_preformatted_block_leaves_an_exact_edge_glyph_outside() {
    let (_, entities) = compose::insert_emoji(
        "alpha",
        &[json!({"type":"pre","offset":0,"length":5,"language":"rust"})],
        1,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    assert_eq!(entities[0]["length"], 5);
    assert_eq!(entities[0]["language"], "rust");
}
#[test]
fn insert_code_block_leaves_an_exact_edge_glyph_outside() {
    let (_, entities) = compose::insert_emoji(
        "alpha",
        &[json!({"_":"messageEntityCode","offset":0,"length":5})],
        1,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    assert_eq!(entities[0]["length"], 5);
}

#[test]
fn insert_refuses_custom_emoji_inside_code_and_pre_ranges() {
    for kind in ["code", "pre"] {
        assert!(
            compose::insert_emoji(
                "alpha\nbeta",
                &[json!({"type":kind,"offset":0,"length":10})],
                2,
                "🔥",
                "9",
                "start"
            )
            .is_err()
        );
        let (_, entities) = compose::insert_emoji(
            "alpha",
            &[json!({"type":kind,"offset":0,"length":5})],
            1,
            "🔥",
            "9",
            "start",
        )
        .unwrap();
        assert_eq!(
            entities.iter().find(|e| e["type"] == kind).unwrap()["offset"],
            2
        );
    }
}
#[test]
fn insert_inline_link_never_swallows_the_new_emoji() {
    let (_, entities) = compose::insert_emoji(
        "alpha\nbeta",
        &[json!({"type":"text_url","offset":6,"length":4,"url":"https://example.org"})],
        2,
        "🔥",
        "9",
        "end",
    )
    .unwrap();
    assert_eq!(entities[0]["length"], 4);
}
#[test]
fn insert_refuses_unknown_line_by_name() {
    let error = compose::insert_emoji("a\nb", &[], 9, "x", "1", "end")
        .unwrap_err()
        .to_string();
    assert!(error.contains("2 line"));
}
#[test]
fn insert_long_id_leaves_as_a_decimal_string() {
    let (_, entities) =
        compose::insert_emoji("a", &[], 1, "✅", "5776121630275149735", "end").unwrap();
    assert_eq!(entities[0]["custom_emoji_id"], "5776121630275149735");
}
#[test]
fn insert_validates_existing_surrogate_boundaries() {
    assert!(
        compose::insert_emoji(
            "😀a",
            &[json!({"type":"bold","offset":1,"length":1})],
            1,
            "✅",
            "1",
            "end"
        )
        .is_err()
    );
}
#[test]
fn insert_preserves_persian_joiners_and_a_compound_fallback() {
    let (text, entities) = compose::insert_emoji("می‌کند", &[], 1, "👨‍👩‍👧", "1", "start").unwrap();
    assert_eq!(text, "👨‍👩‍👧می‌کند");
    assert_eq!(entities[0]["length"], 8);
}
#[test]
fn insert_refuses_invalid_mode_and_empty_fallback() {
    assert!(compose::insert_emoji("a", &[], 1, "", "1", "end").is_err());
    assert!(compose::insert_emoji("a", &[], 1, "a", "1", "middle").is_err());
}
#[test]
fn visible_frame_choice_ignores_blank_endpoints() {
    let frames = vec![
        json!({"blank":true,"bytes":9000}),
        json!({"blank":false,"bytes":3000}),
        json!({"blank":true,"bytes":10000}),
    ];
    assert_eq!(compose::best_frame(&frames), 1);
}
#[test]
fn visible_frame_choice_uses_largest_nonblank_frame() {
    assert_eq!(
        compose::best_frame(&[
            json!({"blank":true}),
            json!({"bytes":900}),
            json!({"bytes":4200})
        ]),
        2
    );
}
#[test]
fn visible_frame_choice_accepts_native_transparent_metadata() {
    assert_eq!(
        compose::best_frame(&[
            json!({"transparent":true,"bytes":9000}),
            json!({"transparent":false,"bytes":40})
        ]),
        1
    );
}
#[test]
fn visible_frame_choice_handles_all_blank_and_empty_inputs() {
    assert_eq!(
        compose::best_frame(&[json!({"blank":true}), json!({"blank":true})]),
        0
    );
    assert_eq!(compose::best_frame(&[]), 0);
}
#[test]
fn describe_lines_recognises_native_and_published_emoji_entities() {
    let entities = vec![
        json!({"_":"messageEntityCustomEmoji","offset":2,"length":2,"document_id":"9007199254740993"}),
        json!({"type":"custom_emoji","offset":0,"length":1,"custom_emoji_id":"2"}),
    ];
    let rows = compose::describe_lines("a\n😀", &entities).unwrap();
    assert_eq!(rows[0]["emoji"], json!(["2"]));
    assert_eq!(rows[1]["emoji"], json!(["9007199254740993"]));
}
#[test]
fn utf16_line_lookup_counts_trailing_empty_lines() {
    assert_eq!(compose::line_of_offset("😀\na\n", 5).unwrap(), 3);
    assert!(compose::line_of_offset("😀", 1).is_err());
}

#[test]
fn studio_payload_groups_each_documents_actual_frames_without_reordering() {
    use base64::Engine as _;
    use telegram_mcp::emoji::studio::{ToolReply, group_frames};
    let content = vec![
        json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(b"first")}),
        json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(b"second")}),
        json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(b"third")}),
        json!({"type":"text","text":json!({"metadata":{"emoji":[{"document_id":"9007199254740993","preview":[{"blank":true},{"blank":false}]},{"document_id":"9007199254740994","preview":[{}]}]}}).to_string()}),
    ];
    let reply = ToolReply::parse(json!({"content":content})).unwrap();
    let (frames, _) = group_frames(&reply).unwrap();
    assert_eq!(frames["9007199254740993"][1].0, b"second");
    assert_eq!(frames["9007199254740994"][0].0, b"third");
}
#[test]
fn studio_refuses_image_count_mismatches_and_mcp_errors() {
    use telegram_mcp::emoji::studio::{ToolReply, group_frames};
    let reply=ToolReply::parse(json!({"structuredContent":{"metadata":{"emoji":[{"document_id":"1","preview":[{},{}]}]}},"content":[]})).unwrap();
    assert!(group_frames(&reply).is_err());
    assert!(ToolReply::parse(json!({"isError":true,"content":[{"text":"refused"}]})).is_err());
}
