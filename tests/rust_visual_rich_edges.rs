use serde_json::{Value, json};
use telegram_mcp::export::rich;

fn text(kind: &str, body: Value) -> Value {
    json!({"_":kind,"text":body})
}
fn render(blocks: Value, files: Value) -> String {
    rich::message(&json!({"blocks":blocks}), &files).unwrap()
}

#[test]
fn mixed_typography_preserves_semantic_nesting_and_escapes_each_leaf() {
    let content = text(
        "textBold",
        text(
            "textItalic",
            text(
                "textUnderline",
                text(
                    "textStrike",
                    text(
                        "textFixed",
                        text(
                            "textSubscript",
                            text("textSuperscript", text("textMarked", json!("<x> 😀"))),
                        ),
                    ),
                ),
            ),
        ),
    );
    assert_eq!(
        rich::text(&content, &Value::Null, 0).unwrap(),
        "<strong><em><u><s><code><sub><sup><mark>&lt;x&gt; 😀</mark></sup></sub></code></s></u></em></strong>"
    );
    assert_eq!(rich::text(&Value::Null, &Value::Null, 0).unwrap(), "");
    assert_eq!(
        rich::text(&json!({"_":"textEmpty"}), &Value::Null, 0).unwrap(),
        ""
    );
    assert_eq!(
        rich::text(&json!("<&"), &Value::Null, 0).unwrap(),
        "&lt;&amp;"
    );
}

#[test]
fn rich_links_email_and_phone_are_clickable_only_with_safe_targets() {
    let value = json!({"_":"textConcat","texts":[
        {"_":"textUrl","url":"https://example.org/?x=1&y=2","text":"site"},
        {"_":"textEmail","email":"owner@example.org","text":"email"},
        {"_":"textPhone","phone":"+12025550123","text":"phone"},
        {"_":"textUrl","url":"data:text/html,<script>","text":"unsafe"}
    ]});
    let html = rich::text(&value, &Value::Null, 0).unwrap();
    assert!(html.contains("href=\"https://example.org/?x=1&amp;y=2\""));
    assert!(html.contains("href=\"mailto:owner@example.org\""));
    assert!(html.contains("href=\"tel:+12025550123\""));
    assert!(html.ends_with("unsafe"));
    assert_eq!(html.matches("<a ").count(), 3);
    assert_eq!(html.matches("noreferrer noopener").count(), 3);
}

#[test]
fn rich_dates_math_annotations_and_future_wrappers_keep_readable_text() {
    let value = json!({"_":"textConcat","texts":[
        {"_":"textDate","date":1700000000,"text":"today"},
        {"_":"textMath","source":"a < b & c"},
        {"_":"textMentionName","user_id":"7","text":"Sara"},
        {"_":"textButton","text":"read-only label"},
        {"_":"futureTextWrapper","text":{"_":"textHashtag","text":"#خبر"}},
        {"_":"unrecognizedLeaf","payload":"<hidden>"}
    ]});
    let html = rich::text(&value, &Value::Null, 0).unwrap();
    assert!(html.contains("<time data-unix=\"1700000000\">today</time>"));
    assert!(html.contains("<code class=\"math\">a &lt; b &amp; c</code>"));
    assert!(html.contains("Sararead-only label#خبر"));
    assert!(html.contains("unrecognizedLeaf"));
    assert!(!html.contains("<hidden>"));
    assert!(!html.contains("<button"));
}

#[test]
fn rich_document_outline_preserves_heading_levels_author_quotes_and_source() {
    let html = render(
        json!([
            {"_":"pageBlockTitle","text":"Title"},{"_":"pageBlockHeader","text":"Chapter"},
            {"_":"pageBlockSubheader","text":"Section"},{"_":"pageBlockHeading4","text":"Subsection"},
            {"_":"pageBlockHeading5","text":"Detail"},{"_":"pageBlockHeading6","text":"Leaf"},
            {"_":"pageBlockAuthorDate","author":"Author <one>","published_date":1700000000},
            {"_":"pageBlockPullquote","text":"Quoted","caption":"Source"},
            {"_":"pageBlockBlockquoteBlocks","blocks":[{"_":"pageBlockParagraph","text":"Nested"}],"caption":"Credit"},
            {"_":"pageBlockPreformatted","text":"let x = <tag>;"},{"_":"pageBlockDivider"},
            {"_":"pageBlockFooter","text":"Footer"},{"_":"pageBlockMath","source":"<math> & x"},{"_":"pageBlockAnchor","name":"internal"}
        ]),
        Value::Null,
    );
    assert!(html.starts_with("<h1>Title</h1><h2>Chapter</h2><h3>Section</h3><h4>Subsection</h4><h5>Detail</h5><h6>Leaf</h6>"));
    assert!(html.contains("Author &lt;one&gt; — 1700000000"));
    assert!(html.contains("<blockquote>Quoted<footer>Source</footer></blockquote>"));
    assert!(html.contains("<blockquote><p>Nested</p>Credit</blockquote>"));
    assert!(html.contains("<pre>let x = &lt;tag&gt;;</pre><hr><p>Footer</p>"));
    assert!(html.ends_with("<pre class=\"math\">&lt;math&gt; &amp; x</pre>"));
}

#[test]
fn ordered_reversed_list_keeps_nested_blocks_and_unchecked_items() {
    let html = render(
        json!([{"_":"pageBlockOrderedList","start":3,"reversed":true,"items":[
            {"checkbox":true,"checked":false,"text":"Pending"},
            {"blocks":[{"_":"pageBlockParagraph","text":"Line one"},{"_":"pageBlockParagraph","text":"Line two"}]}
        ]}]),
        Value::Null,
    );
    assert_eq!(
        html,
        "<ol start=\"3\" reversed><li>☐ Pending</li><li><p>Line one</p><p>Line two</p></li></ol>"
    );
}

#[test]
fn cover_collage_and_slideshow_keep_media_captions_and_credit() {
    let html = render(
        json!([
            {"_":"pageBlockCover","cover":{"_":"pageBlockPhoto","photo_id":7,"caption":{"text":"Cover","credit":"Photographer"}}},
            {"_":"pageBlockCollage","items":[{"_":"pageBlockPhoto","photo_id":8}],"caption":{"text":"Collage"}},
            {"_":"pageBlockSlideshow","items":[{"_":"pageBlockParagraph","text":"Slide"}],"caption":{"credit":"Designer"}}
        ]),
        json!({"7":"photos/cover #1.jpg","8":"photos/grid.jpg"}),
    );
    assert!(html.contains("src=\"photos/cover%20%231.jpg\""));
    assert!(html.contains("<figcaption>CoverPhotographer</figcaption>"));
    assert!(html.contains("<section><figure>"));
    assert!(html.contains("<figcaption>Collage</figcaption>"));
    assert!(html.contains("<section><p>Slide</p><figcaption>Designer</figcaption></section>"));
}

#[test]
fn video_audio_document_and_inline_image_keep_exact_media_identity() {
    let files = json!({"11":"video_files/clip.mp4","12":"files/song.ogg","13":"files/document.pdf","14":"photos/icon.webp","15":"stickers/emoji.webp"});
    let html = render(
        json!([
            {"_":"pageBlockVideo","video_id":11,"caption":{"text":"Movie"}},
            {"_":"pageBlockAudio","audio_id":"12","caption":{"credit":"Performer"}},
            {"_":"pageBlockDocument","document_id":"13"},
            {"_":"pageBlockParagraph","text":{"_":"textImage","document_id":14}},
            {"_":"pageBlockParagraph","text":{"_":"textCustomEmoji","document_id":"15","alt":"😀"}},
            {"_":"pageBlockDocument","document_id":99}
        ]),
        files,
    );
    assert!(html.contains("href=\"video_files/clip.mp4\">Media 11"));
    assert!(html.contains("href=\"files/song.ogg\">Media 12"));
    assert!(html.contains("href=\"files/document.pdf\">Media 13"));
    assert!(html.contains("src=\"photos/icon.webp\""));
    assert!(html.contains("src=\"stickers/emoji.webp\""));
    assert!(html.contains("Media 99 (not downloaded)"));
    assert!(!html.contains("<iframe"));
}

#[test]
fn table_span_bounds_and_default_data_cells_cannot_inject_attributes() {
    let html = render(
        json!([{"_":"pageBlockTable","title":"<Data>","rows":[{"cells":[
            {"header":false,"colspan":0,"rowspan":10000,"text":"<cell>"},
            {"text":"second"}
        ]}]}]),
        Value::Null,
    );
    assert!(html.contains("<caption>&lt;Data&gt;</caption>"));
    assert!(html.contains("<td colspan=\"1\" rowspan=\"100\">&lt;cell&gt;</td>"));
    assert!(html.contains("<td colspan=\"1\" rowspan=\"1\">second</td>"));
}

#[test]
fn nested_details_and_safe_embed_preserve_reading_without_executing_html() {
    let html = render(
        json!([
            {"_":"pageBlockDetails","open":true,"title":"Expanded","blocks":[{"_":"pageBlockParagraph","text":"Visible"}]},
            {"_":"pageBlockDetails","title":"Collapsed","blocks":[]},
            {"_":"pageBlockEmbedPost","url":"https://example.org/?x=1&y=2","html":"<iframe src='bad'></iframe>","blocks":[{"_":"pageBlockParagraph","text":"Archived"}]}
        ]),
        Value::Null,
    );
    assert!(html.starts_with("<details open><summary>Expanded</summary><p>Visible</p></details><details><summary>Collapsed</summary></details>"));
    assert!(html.contains("href=\"https://example.org/?x=1&amp;y=2\""));
    assert!(html.contains("&lt;iframe"));
    assert!(!html.contains("<iframe"));
    assert!(html.ends_with("<p>Archived</p>"));
}

#[test]
fn recursive_block_containers_are_refused_before_stack_exhaustion() {
    let mut value = json!({"_":"pageBlockParagraph","text":"end"});
    for _ in 0..70 {
        value = json!({"_":"pageBlockCover","cover":value});
    }
    assert!(
        rich::block(&value, &Value::Null, 0)
            .unwrap_err()
            .to_string()
            .contains("64")
    );
}
