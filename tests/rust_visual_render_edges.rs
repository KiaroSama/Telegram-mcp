use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::export::render;

fn entity(kind: &str, text: &str) -> String {
    render::render_entities(
        text,
        &[json!({"_":kind,"offset":0,"length":text.encode_utf16().count()})],
    )
    .unwrap()
}

#[test]
fn exported_inline_and_block_entities_keep_semantics_without_executable_markup() {
    assert_eq!(entity("messageEntityUnderline", "a<b"), "<u>a&lt;b</u>");
    assert_eq!(entity("messageEntityStrike", "removed"), "<s>removed</s>");
    assert_eq!(
        entity("messageEntityCode", "<code>"),
        "<code>&lt;code&gt;</code>"
    );
    assert_eq!(entity("messageEntityPre", "x\ny"), "<pre>x<br>y</pre>");
    assert_eq!(
        entity("messageEntityBlockquote", "quote"),
        "<blockquote>quote</blockquote>"
    );
}

#[test]
fn exported_mentions_email_and_plain_url_links_use_only_valid_targets() {
    assert_eq!(
        entity("messageEntityMention", "@sample_user"),
        "<a href=\"https://t.me/sample_user\">@sample_user</a>"
    );
    assert_eq!(
        entity("messageEntityMention", "@unsafe/user"),
        "@unsafe/user"
    );
    assert_eq!(
        entity("messageEntityEmail", "a@example.org"),
        "<a href=\"mailto:a@example.org\">a@example.org</a>"
    );
    assert_eq!(
        entity("messageEntityEmail", "bad@@example.org"),
        "bad@@example.org"
    );
    assert!(
        entity("messageEntityUrl", "https://example.org/x")
            .contains("href=\"https://example.org/x\"")
    );
    assert_eq!(entity("futureEntity", "visible"), "visible");
}

#[test]
fn mail_and_telephone_links_refuse_query_injection_non_numeric_paths_and_hidden_controls() {
    assert_eq!(
        render::safe_href("mailto:owner@example.org"),
        Some("mailto:owner@example.org".into())
    );
    assert_eq!(
        render::safe_href("tel:+1-202-555-0123"),
        Some("tel:+1-202-555-0123".into())
    );
    for url in [
        "mailto:owner@example.org?bcc=other@example.org",
        "mailto:a@@b.org",
        "mailto:missing",
        "tel:abc",
        "tel:123;ext=3",
        "https://example.org/%1B",
        "https://example.org/%09",
    ] {
        assert!(render::safe_href(url).is_none(), "Unsafe target {url}");
    }
}

#[test]
fn zero_length_entities_are_ignored_but_malformed_ranges_are_refused() {
    assert_eq!(
        render::render_entities(
            "😀x",
            &[json!({"_":"messageEntityBold","offset":2,"length":0})]
        )
        .unwrap(),
        "😀x"
    );
    for bad in [
        json!({"_":"messageEntityBold","offset":-1,"length":1}),
        json!({"_":"messageEntityBold","offset":0,"length":"1"}),
        json!({"_":"messageEntityBold","offset":u64::MAX,"length":1}),
    ] {
        assert!(render::render_entities("x", &[bad]).is_err());
    }
}

#[test]
fn custom_emoji_file_links_preserve_numeric_ids_and_refuse_path_escape() {
    let entity = json!({"_":"messageEntityCustomEmoji","offset":0,"length":2,"document_id":123});
    assert!(
        render::render_entities_with_files(
            "😀",
            std::slice::from_ref(&entity),
            &json!({"123":"stickers/a #1.webp"})
        )
        .unwrap()
        .contains("stickers/a%20%231.webp")
    );
    assert_eq!(
        render::render_entities_with_files("😀", &[entity], &json!({"123":"../../private"}))
            .unwrap(),
        "😀"
    );
}

#[test]
fn unknown_senders_and_unavailable_media_are_readable_without_literal_nulls() {
    assert_eq!(
        render::person_name(&json!({"username":"known_name"})),
        "known_name"
    );
    assert_eq!(render::person_name(&json!({})), "Unknown user");
    let html=render::html_message(&json!({"_":"message","id":1,"date":1700000000,"message":"text","media":{"_":"messageMediaDocument"},"export_file_skipped":"not selected"}),&BTreeMap::new()).unwrap();
    assert!(html.contains("text"));
    assert!(!html.contains("href="));
    assert!(!html.contains("null"));
    let value: Value = serde_json::from_str(
        &serde_json::to_string(
            &render::json_message(
                &json!({"_":"message","id":2,"message":"x","reply_to":{"reply_to_msg_id":5}}),
                &BTreeMap::new(),
            )
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["reply_to"]["reply_to_msg_id"], 5);
}
