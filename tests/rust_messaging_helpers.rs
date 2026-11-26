use serde_json::{Value, json};
use telegram_mcp::tools::messaging::helpers;
fn bold(offset: i64, length: i64) -> Value {
    json!({"type":"bold","offset":offset,"length":length})
}

#[test]
fn custom_emoji_rebuild_preserves_exact_64_bit_id() {
    let e=helpers::entities("hello",&json!([{"type":"custom_emoji","offset":0,"length":5,"custom_emoji_id":"5934007978150595964"}])).unwrap();
    assert_eq!(e[0]["document_id"], "5934007978150595964");
    assert_eq!(e[0]["_"], "messageEntityCustomEmoji");
}
#[test]
fn mixed_entities_survive_without_reordering() {
    let e = helpers::entities(
        "hello world",
        &json!([{"type":"custom_emoji","offset":0,"length":5,"custom_emoji_id":"555"},bold(6,5)]),
    )
    .unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[1]["_"], "messageEntityBold");
}
#[test]
fn offset_beyond_text_is_refused() {
    assert!(helpers::entities("text", &json!([bold(5, 1)])).is_err());
}
#[test]
fn raw_offset_is_refused() {
    assert!(
        helpers::entities(
            "text",
            &json!([{"type":"bold","offset":0,"length":4,"offset_is_raw":true}])
        )
        .unwrap_err()
        .to_string()
        .contains("Raw")
    );
}
#[test]
fn entity_cannot_split_surrogate_start() {
    assert!(helpers::entities("😀a", &json!([bold(1, 1)])).is_err());
}
#[test]
fn entity_cannot_split_surrogate_end() {
    assert!(helpers::entities("😀a", &json!([bold(0, 1)])).is_err());
}
#[test]
fn astral_entity_uses_utf16_units() {
    let e = helpers::entities("😀a", &json!([bold(2, 1)])).unwrap();
    assert_eq!(e[0]["offset"], 2);
}
#[test]
fn empty_entity_length_is_refused() {
    assert!(helpers::entities("a", &json!([bold(0, 0)])).is_err());
}
#[test]
fn boolean_entity_offset_is_refused() {
    assert!(helpers::entities("a", &json!([{"type":"bold","offset":true,"length":1}])).is_err());
}
#[test]
fn altered_url_is_refused() {
    assert!(
        helpers::entities(
            "a",
            &json!([{"type":"text_url","offset":0,"length":1,"url":"x","url_altered":true}])
        )
        .is_err()
    );
}
#[test]
fn preformatted_entity_keeps_language() {
    let e = helpers::entities(
        "a",
        &json!([{"type":"pre","offset":0,"length":1,"language":"Rust"}]),
    )
    .unwrap();
    assert_eq!(e[0]["language"], "Rust");
}
#[test]
fn entity_display_text_is_not_a_wire_field() {
    let e = helpers::entities(
        "a",
        &json!([{"type":"bold","offset":0,"length":1,"text":"a"}]),
    )
    .unwrap();
    assert!(e[0].get("text").is_none());
}
#[test]
fn ordinary_markdown_preserves_astral_offsets() {
    let (text, e) = telegram_mcp::backend::parse_text("😀 **hello**", Some("markdown")).unwrap();
    assert_eq!(text, "😀 hello");
    assert_eq!(e[0]["offset"], 3);
    assert_eq!(e[0]["length"], 5);
}
#[test]
fn ordinary_html_preserves_entities() {
    let (text, e) =
        telegram_mcp::backend::parse_text("<b>hello</b> &amp; <i>world</i>", Some("html")).unwrap();
    assert_eq!(text, "hello & world");
    assert_eq!(e.len(), 2);
}
#[test]
fn malformed_date_is_refused() {
    assert!(helpers::date(&json!({"from_date":"05/01/2026"}), "from_date", false).is_err());
}
#[test]
fn to_date_is_exclusive_next_midnight() {
    assert_eq!(
        helpers::date(&json!({"to_date":"2026-05-15"}), "to_date", true).unwrap(),
        1778889600
    );
}
#[test]
fn repeats_are_only_supported_native_periods() {
    assert_eq!(
        helpers::repeat(&json!({"repeat":"weekly"})).unwrap(),
        Some(604800)
    );
    assert!(helpers::repeat(&json!({"repeat":"hourly"})).is_err());
}
#[test]
fn scheduling_rejects_past_time() {
    assert!(helpers::schedule(&json!({"when":1}), "when").is_err());
}
#[test]
fn scheduling_understands_timezone_offset() {
    let time = helpers::schedule(&json!({"when":"2030-01-01T03:30:00+03:30"}), "when").unwrap();
    assert_eq!(time, Some(1893456000));
}
#[test]
fn media_location_retains_the_documents_datacenter() {
    let location=telegram_mcp::tools::messaging::media::download_location(&json!({"_":"messageMediaDocument","document":{"_":"document","id":"9007199254740995","access_hash":"919191919191919191","file_reference":[1,2],"dc_id":4,"size":10}})).unwrap();
    assert_eq!(location["_"], "inputDocumentFileLocation");
    assert_eq!(location["id"], "9007199254740995");
    assert_eq!(location["_dc_id"], 4);
    assert_eq!(location["thumb_size"], "");
}
#[test]
fn photo_location_selects_the_largest_size_reference() {
    let location=telegram_mcp::tools::messaging::media::download_location(&json!({"_":"photo","id":"9","access_hash":"13","file_reference":[],"dc_id":2,"sizes":[{"type":"s"},{"type":"x"}]})).unwrap();
    assert_eq!(location["_"], "inputPhotoFileLocation");
    assert_eq!(location["thumb_size"], "x");
}
#[test]
fn media_without_a_retrievable_reference_is_refused() {
    assert!(
        telegram_mcp::tools::messaging::media::download_location(
            &json!({"_":"messageMediaUnsupported"})
        )
        .is_err()
    );
}
#[test]
fn file_size_reads_decimal_64_bit_sizes_without_float_rounding() {
    assert_eq!(
        telegram_mcp::tools::messaging::media::downloads::declared_size(
            &json!({"document":{"size":"9007199254740995"}})
        ),
        Some(9007199254740995)
    );
}
#[test]
fn photo_progressive_sizes_contribute_the_largest_declared_budget() {
    assert_eq!(
        telegram_mcp::tools::messaging::media::downloads::declared_size(
            &json!({"photo":{"sizes":[{"sizes":[10,30,90]},{"size":50}]}})
        ),
        Some(90)
    );
}
#[test]
fn static_preview_returns_real_encoded_image_content() {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 4)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let preview = telegram_mcp::tools::messaging::media::previews::image_content(
        &bytes.into_inner(),
        256,
        json!({}),
    )
    .unwrap();
    assert_eq!(preview["content"][0]["type"], "image");
    assert_eq!(preview["content"][0]["mimeType"], "image/png");
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(preview["content"][0]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(image::load_from_memory(&data).unwrap().width(), 8);
}
#[test]
fn stripped_thumbnail_is_reconstructed_with_real_jpeg_header() {
    let bytes = telegram_mcp::tools::messaging::media::previews::inline_thumbnail(
        &json!({"_":"photoStrippedSize","type":"i","bytes":[1,3,4,77,88]}),
    )
    .unwrap();
    assert_eq!(&bytes[..2], &[255, 216]);
    assert_eq!(&bytes[bytes.len() - 2..], &[255, 217]);
    assert_eq!(bytes[164], 3);
    assert_eq!(bytes[166], 4);
}
#[test]
fn invalid_stripped_thumbnail_is_refused() {
    assert!(
        telegram_mcp::tools::messaging::media::previews::inline_thumbnail(
            &json!({"_":"photoStrippedSize","type":"i","bytes":[9,3,4]})
        )
        .is_err()
    );
}

#[test]
fn formatted_dates_and_entity_errors_cover_the_whole_request() {
    let parsed=helpers::entities("x",&json!([{"type":"formatted_date","offset":0,"length":1,"date":"1970-01-01T02:00:01+02:00"}])).unwrap();
    assert_eq!(parsed[0]["date"], 1);
    let err = helpers::entities(
        "x",
        &json!([bold(2,1),{"type":"invalid","offset":0,"length":1}]),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("entities[0]") && err.contains("entities[1]"));
}

#[test]
fn download_destination_rejects_tilde_expansion_but_allows_windows_short_names() {
    let temp = tempfile::tempdir().unwrap();
    let short_dir = temp.path().join("RUNNER~1").join("out");
    std::fs::create_dir_all(&short_dir).unwrap();
    let state =
        telegram_mcp::state::State::at(temp.path().join("state"), vec![short_dir.clone()]).unwrap();

    let tilde_err = telegram_mcp::tools::messaging::media::downloads::destination_in(
        &state,
        Some("~/secret.bin"),
        &json!({}),
        1,
    )
    .unwrap_err();
    assert!(tilde_err.to_string().to_lowercase().contains("tilde"));

    let nested_tilde = short_dir.join("~secret.bin");
    let nested_err = telegram_mcp::tools::messaging::media::downloads::destination_in(
        &state,
        Some(&nested_tilde.to_string_lossy()),
        &json!({}),
        1,
    )
    .unwrap_err();
    assert!(
        nested_err
            .to_string()
            .contains("Tilde is not permitted in download paths")
    );

    let short_dest = short_dir.join("fixture.bin");
    let resolved = telegram_mcp::tools::messaging::media::downloads::destination_in(
        &state,
        Some(&short_dest.to_string_lossy()),
        &json!({}),
        1,
    )
    .unwrap();
    assert_eq!(resolved.file_name().unwrap(), "fixture.bin");
}
