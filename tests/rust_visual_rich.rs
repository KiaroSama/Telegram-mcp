use serde_json::{Value, json};
use telegram_mcp::export::rich;

#[test]
fn rich_paragraph_formats_nested_text_without_losing_unicode() {
    let value = json!({"blocks":[{"_":"pageBlockParagraph","text":{"_":"textConcat","texts":[{"_":"textPlain","text":"سلام "},{"_":"textBold","text":{"_":"textPlain","text":"😀"}}]}}]});
    assert_eq!(
        rich::message(&value, &Value::Null).unwrap(),
        "<p>سلام <strong>😀</strong></p>"
    );
}
#[test]
fn rich_embed_never_executes_server_supplied_html() {
    let value = json!({"blocks":[{"_":"pageBlockEmbed","html":"<script>alert(1)</script>","url":"javascript:alert(1)"}]});
    let html = rich::message(&value, &Value::Null).unwrap();
    assert!(!html.contains("<script>"));
    assert!(!html.contains("href="));
    assert!(html.contains("&lt;script&gt;"));
}
#[test]
fn rich_table_renders_header_spans_and_unicode() {
    let value = json!({"blocks":[{"_":"pageBlockTable","title":{"_":"textPlain","text":"Data"},"rows":[{"cells":[{"header":true,"colspan":2,"text":{"_":"textPlain","text":"ستون"}}]}]}]});
    let html = rich::message(&value, &Value::Null).unwrap();
    assert!(html.contains("<caption>Data</caption>"));
    assert!(html.contains("<th colspan=\"2\" rowspan=\"1\">ستون</th>"));
}
#[test]
fn rich_checklist_keeps_checked_state() {
    let value = json!({"blocks":[{"_":"pageBlockList","items":[{"_":"pageListItemText","checkbox":true,"checked":true,"text":{"_":"textPlain","text":"Done"}}]}]});
    assert_eq!(
        rich::message(&value, &Value::Null).unwrap(),
        "<ul><li>☑ Done</li></ul>"
    );
}
#[test]
fn rich_photo_uses_only_safe_relative_downloaded_path() {
    let value = json!({"blocks":[{"_":"pageBlockPhoto","photo_id":"9007199254740993","caption":{"text":{"_":"textPlain","text":"Picture"}}}]});
    let html = rich::message(&value, &json!({"9007199254740993":"photos/image.jpg"})).unwrap();
    assert!(html.contains("src=\"photos/image.jpg\""));
    assert!(html.contains("Picture"));
    let unsafe_html =
        rich::message(&value, &json!({"9007199254740993":"../../private.jpg"})).unwrap();
    assert!(!unsafe_html.contains("src="));
}
#[test]
fn rich_custom_emoji_keeps_fallback_when_asset_is_unavailable() {
    assert_eq!(
        rich::text(
            &json!({"_":"textCustomEmoji","document_id":"123","alt":"😀"}),
            &Value::Null,
            0
        )
        .unwrap(),
        "😀"
    );
}
#[test]
fn rich_unknown_blocks_remain_visible_and_escaped() {
    let html = rich::message(
        &json!({"blocks":[{"_":"newBlock","caption":"<unsafe>"}]}),
        &Value::Null,
    )
    .unwrap();
    assert!(html.contains("newBlock"));
    assert!(html.contains("&lt;unsafe&gt;"));
}
#[test]
fn rich_recursion_is_bounded_before_stack_exhaustion() {
    let mut text = json!({"_":"textPlain","text":"x"});
    for _ in 0..100 {
        text = json!({"_":"textBold","text":text});
    }
    assert!(
        rich::text(&text, &Value::Null, 0)
            .unwrap_err()
            .to_string()
            .contains("64")
    );
}
#[test]
fn rich_diff_preserves_before_and_after() {
    let html=rich::text(&json!({"_":"textDiff","old_text":{"_":"textPlain","text":"old"},"text":{"_":"textPlain","text":"new"}}),&Value::Null,0).unwrap();
    assert_eq!(html, "<del>old</del><ins>new</ins>");
}
#[test]
fn rich_spoilers_use_native_html_disclosure_without_javascript() {
    let html = rich::text(
        &json!({"_":"textSpoiler","text":{"_":"textPlain","text":"hidden"}}),
        &Value::Null,
        0,
    )
    .unwrap();
    assert!(html.starts_with("<details>"));
    assert!(!html.contains("onclick"));
}
