use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::export::{files, render, rich};

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
    let html = render::html_message(
        &json!({"_":"message","id":1,"date":1700000000,"message":"text","media":{"_":"messageMediaDocument"},"export_file_skipped":"not selected"}),
        &BTreeMap::new(),
    )
    .unwrap();
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

#[test]
fn archive_references_reject_absolute_paths_host_backslashes_and_bidi_controls() {
    for bad in [
        "/etc/passwd",
        "C:\\Users\\private\\file.jpg",
        "photos\\nested.jpg",
        "photos/../secret.jpg",
        "./photos/a.jpg",
        "photos/a\u{202E}gpj.exe",
        "file:///tmp/a.jpg",
    ] {
        assert!(
            files::clean_relative_path(bad).is_none(),
            "Must reject {bad}"
        );
        assert!(render::relative_href(bad).is_none(), "Must reject {bad}");
    }
    assert_eq!(
        files::clean_relative_path("contacts/contact_1.vcard"),
        Some("contacts/contact_1.vcard".into())
    );

    let msg = json!({
        "_": "message",
        "id": 9,
        "date": 1700000000,
        "message": "spoof \u{202E}txt.exe <script>alert(1)</script>",
        "export_file": "C:\\Users\\owner\\secret.pdf",
        "export_thumb": "../outside_thumb.jpg",
        "contact_vcard": "\\\\server\\share\\card.vcard",
        "media": {
            "_": "messageMediaContact",
            "first_name": "Evil\u{202E}<img src=x onerror=alert(1)>",
            "last_name": "Contact",
            "phone_number": "+12025550100"
        }
    });
    let json_rec = render::json_message(&msg, &BTreeMap::new()).unwrap();
    assert!(json_rec.get("file").is_none());
    assert!(json_rec.get("thumbnail").is_none());
    assert!(json_rec.get("contact_vcard").is_none());
    assert!(
        !json_rec["contact_information"]["first_name"]
            .as_str()
            .unwrap()
            .contains('\u{202E}')
    );
    let html = render::html_message(&msg, &BTreeMap::new()).unwrap();
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains('\u{202E}'));
    assert!(!html.contains("secret.pdf"));
    assert!(!html.contains("outside_thumb"));
}

#[test]
fn safe_text_projection_rebases_utf16_entities_after_stripping_unsafe_controls() {
    let raw_text = "\u{202E}\u{0007}سلام 😀 bold";
    let entities = vec![json!({
        "_": "messageEntityBold",
        "offset": 10,
        "length": 4
    })];
    let projected = render::project_safe_text(raw_text, &entities);
    assert_eq!(projected.text, "سلام 😀 bold");
    assert_eq!(projected.entities.len(), 1);
    assert_eq!(projected.entities[0].offset, 8);
    assert_eq!(projected.entities[0].length, 4);
}

#[test]
fn rich_channel_related_articles_and_document_thumbnails_render_without_scripts() {
    let rich_doc = json!({
        "blocks": [
            {
                "_": "pageBlockChannel",
                "channel": {"title": "News <Feed>", "username": "telegram"}
            },
            {
                "_": "pageBlockRelatedArticles",
                "title": "Further reading",
                "articles": [
                    {"title": "Safe", "description": "Summary", "url": "https://example.org/a"},
                    {"title": "Blocked", "description": "XSS", "url": "javascript:alert(1)"}
                ]
            },
            {
                "_": "pageBlockVideo",
                "video_id": "77",
                "caption": {"text": "Clip"}
            }
        ]
    });
    let html = rich::message_with_thumbs(
        &rich_doc,
        &json!({"77": "video_files/77.mp4"}),
        &json!({"77": "video_files/77_thumb.jpg"}),
    )
    .unwrap();
    assert!(html.contains("href=\"https://t.me/telegram\">News &lt;Feed&gt;</a>"));
    assert!(html.contains("href=\"https://example.org/a\">Safe</a>"));
    assert!(!html.contains("javascript:"));
    assert!(html.contains("src=\"video_files/77_thumb.jpg\""));
    assert!(html.contains("href=\"video_files/77.mp4\""));
}
