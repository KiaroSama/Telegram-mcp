//! Keyboard identity, account binding and rich fragment regressions.
#[path = "common/messaging.rs"]
mod support;
use serde_json::{Value, json};
use support::*;
use telegram_mcp::tools::messaging::{media::uploads, rich_quote};

fn keyboard(data: u8, label: &str) -> Value {
    let mut m = message(10, "");
    m["reply_markup"] = json!({"_":"replyInlineMarkup","rows":[{"_":"keyboardInlineButtonRow","buttons":[{"_":"keyboardInlineButton","text":label,"type":{"_":"inlineButtonTypeCallback","data":[data]}}]}]});
    m
}
#[tokio::test]
async fn callback_requires_the_inspected_identity() {
    let w = Wire::new(vec![]);
    assert!(run(&w,"click_button",json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"go","press_token":"forged"})).await.is_err());
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn callback_with_current_token_sends_the_inspected_bytes() {
    let m = keyboard(99, "go");
    let w = Wire::new(vec![
        messages(vec![m.clone()]),
        messages(vec![m]),
        json!({"message":"done"}),
    ]);
    let listed = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    let token = &listed["buttons"][0]["press_token"];
    let result=run(&w,"click_button",json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"go","press_token":token})).await.unwrap();
    assert_eq!(result["message"], "done");
    assert_eq!(w.calls()[2].0, "messages.getBotCallbackAnswer");
    assert_eq!(w.calls()[2].1["data"], json!([99]));
}
#[tokio::test]
async fn callback_payload_change_invalidates_unchanged_label() {
    let w = Wire::new(vec![
        messages(vec![keyboard(1, "go")]),
        messages(vec![keyboard(2, "go")]),
    ]);
    let listed = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    let result=run(&w,"click_button",json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"go","press_token":listed["buttons"][0]["press_token"]})).await;
    assert!(result.unwrap_err().to_string().contains("changed"));
    assert_eq!(w.calls().len(), 2);
}
#[tokio::test]
async fn callback_token_cannot_cross_account() {
    let w = Wire::new(vec![messages(vec![keyboard(1, "go")])]);
    let listed = run(
        &w,
        "inspect_buttons",
        json!({"chat_id":7,"message_id":10,"account":"alice"}),
    )
    .await
    .unwrap();
    assert!(run(&w,"click_button",json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"go","press_token":listed["buttons"][0]["press_token"],"account":"bob"})).await.is_err());
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn callback_token_cannot_be_replayed_after_a_press() {
    let m = keyboard(1, "go");
    let w = Wire::new(vec![
        messages(vec![m.clone()]),
        messages(vec![m]),
        json!({}),
    ]);
    let listed = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    let args = json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"go","press_token":listed["buttons"][0]["press_token"]});
    run(&w, "click_button", args.clone()).await.unwrap();
    assert!(run(&w, "click_button", args).await.is_err());
    assert_eq!(w.calls().len(), 3);
}
#[tokio::test]
async fn url_button_is_identified_without_a_callback_token() {
    let mut m = keyboard(1, "open");
    m["reply_markup"]["rows"][0]["buttons"][0]["type"] =
        json!({"_":"inlineButtonTypeUrl","url":"https://example.com"});
    let w = Wire::new(vec![messages(vec![m])]);
    let result = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    assert_eq!(result["buttons"][0]["url"], "https://example.com");
    assert!(result["buttons"][0].get("press_token").is_none());
}
#[tokio::test]
async fn button_display_filters_direction_overrides_but_marks_the_change() {
    let w = Wire::new(vec![messages(vec![keyboard(1, "go\u{202e}evil")])]);
    let result = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    assert_eq!(result["buttons"][0]["text"], "goevil");
    assert_eq!(result["buttons"][0]["text_altered"], true);
}
#[tokio::test]
async fn password_callback_is_never_advertised_as_pressable() {
    let mut m = keyboard(1, "go");
    m["reply_markup"]["rows"][0]["buttons"][0]["type"]["requires_password"] = json!(true);
    let w = Wire::new(vec![messages(vec![m])]);
    let result = run(&w, "inspect_buttons", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    assert!(result["buttons"][0].get("press_token").is_none());
}
#[tokio::test]
async fn reply_keyboard_label_collision_refuses_ambiguous_send() {
    let mut m = message(10, "");
    m["reply_markup"] = json!({"_":"replyKeyboardMarkup","rows":[{"buttons":[{"text":"go","type":{"_":"buttonTypeDefault"}},{"text":"go","type":{"_":"buttonTypeRequestPhone"}}]}]});
    let w = Wire::new(vec![messages(vec![m])]);
    assert!(
        run(
            &w,
            "press_reply_button",
            json!({"chat_id":7,"button_index":0,"expect_text":"go","wait_seconds":0})
        )
        .await
        .is_err()
    );
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn reply_keyboard_hide_prevents_old_buttons_being_pressed() {
    let mut m = message(10, "");
    m["reply_markup"] = json!({"_":"replyKeyboardHide"});
    let w = Wire::new(vec![messages(vec![m])]);
    assert!(
        run(
            &w,
            "press_reply_button",
            json!({"chat_id":7,"button_index":0,"expect_text":"go","wait_seconds":0})
        )
        .await
        .is_err()
    );
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn ordinary_reply_button_sends_exact_raw_label() {
    let mut m = message(10, "");
    m["reply_markup"] = json!({"_":"replyKeyboardMarkup","rows":[{"buttons":[{"text":"pay 👨‍👩‍👧","type":{"_":"buttonTypeDefault"}}]}]});
    let w = Wire::new(vec![messages(vec![m]), json!({})]);
    run(
        &w,
        "press_reply_button",
        json!({"chat_id":7,"button_index":0,"expect_text":"pay 👨👩👧","wait_seconds":0}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[1].1["message"], "pay 👨‍👩‍👧");
}
#[tokio::test]
async fn location_request_rejects_invalid_coordinates_before_send() {
    let mut m = message(10, "");
    m["reply_markup"] = json!({"_":"replyKeyboardMarkup","rows":[{"buttons":[{"text":"where","type":{"_":"buttonTypeRequestGeoLocation"}}]}]});
    let w = Wire::new(vec![messages(vec![m])]);
    assert!(run(&w,"answer_reply_button",json!({"chat_id":7,"button_index":0,"expect_text":"where","latitude":91,"longitude":0,"wait_seconds":0})).await.is_err());
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn inline_result_is_bound_to_account() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "send_inline_result",
            json!({"chat_id":7,"account":"bob","result_id":"inline:alice:2000000000:123:one"})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn expired_inline_result_is_refused_before_send() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "send_inline_result",
            json!({"chat_id":7,"result_id":"inline:default:1:123:one"})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
fn rich(nodes: Vec<Value>) -> Value {
    json!({"blocks":nodes})
}
fn paragraph(text: Value) -> Value {
    json!({"_":"pageBlockParagraph","text":text})
}
fn plain(text: &str) -> Value {
    json!({"_":"textPlain","text":text})
}
#[test]
fn rich_quote_preserves_formatting_when_sliced() {
    let source = rich(vec![paragraph(
        json!({"_":"textBold","text":plain("hello world")}),
    )]);
    let (q, offset) = rich_quote::select(&source, &json!({"quote_text":"world"})).unwrap();
    assert_eq!(q[0]["_"], "textBold");
    assert_eq!(q[0]["text"]["text"], "world");
    assert_eq!(offset, 6);
}
#[test]
fn repeated_rich_quote_requires_disambiguating_location() {
    let source = rich(vec![paragraph(plain("same")), paragraph(plain("same"))]);
    assert!(
        rich_quote::select(&source, &json!({"quote_text":"same"}))
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let (_, offset) =
        rich_quote::select(&source, &json!({"quote_text":"same","quote_block_index":1})).unwrap();
    assert_eq!(offset, 5);
}
#[test]
fn table_quote_selects_exact_row_and_column() {
    let source = rich(vec![
        json!({"_":"pageBlockTable","title":plain("table"),"rows":[{"cells":[{"text":plain("left")},{"text":plain("right")}]}]}),
    ]);
    let (q, _) = rich_quote::select(
        &source,
        &json!({"quote_text":"right","quote_block_index":0,"quote_row":0,"quote_column":1}),
    )
    .unwrap();
    assert_eq!(q[0]["text"], "right");
}
#[test]
fn partial_table_coordinates_are_refused() {
    assert!(
        rich_quote::select(
            &rich(vec![paragraph(plain("hello"))]),
            &json!({"quote_text":"hello","quote_row":0})
        )
        .is_err()
    );
}
#[test]
fn rich_quote_uses_utf16_source_offsets() {
    let source = rich(vec![paragraph(plain("😀word"))]);
    let (_, offset) = rich_quote::select(&source, &json!({"quote_text":"word"})).unwrap();
    assert_eq!(offset, 2);
}
#[test]
fn custom_rich_emoji_cannot_be_partially_quoted() {
    let source = rich(vec![paragraph(
        json!({"_":"textCustomEmoji","alt":"hello","document_id":"777"}),
    )]);
    assert!(rich_quote::select(&source, &json!({"quote_text":"ell"})).is_err());
    let (q, _) = rich_quote::select(&source, &json!({"quote_text":"hello"})).unwrap();
    assert_eq!(q[0]["document_id"], "777");
}
#[test]
fn explicit_voice_kind_rejects_mp3() {
    assert!(uploads::validate_kind(std::path::Path::new("voice.mp3"), "voice_note", "").is_err());
}
#[test]
fn sticker_caption_is_refused_instead_of_dropped() {
    assert!(
        uploads::validate_kind(std::path::Path::new("sticker.webp"), "sticker", "caption").is_err()
    );
}
#[test]
fn video_note_caption_is_refused_instead_of_dropped() {
    assert!(
        uploads::validate_kind(std::path::Path::new("clip.mp4"), "video_note", "caption").is_err()
    );
}
#[test]
fn opaque_extension_is_sent_as_document() {
    assert_eq!(
        uploads::infer_kind(std::path::Path::new("data.custom")),
        "document"
    );
}
#[test]
fn ogg_recording_without_music_tags_defaults_to_voice() {
    let temp = tempfile::Builder::new().suffix(".ogg").tempfile().unwrap();
    std::fs::write(temp.path(), b"OggS OpusHead OpusTags").unwrap();
    assert_eq!(uploads::infer_kind(temp.path()), "voice_note");
}
#[test]
fn ogg_with_title_is_music_instead_of_voice() {
    let temp = tempfile::Builder::new().suffix(".ogg").tempfile().unwrap();
    std::fs::write(temp.path(), b"OggS OpusHead OpusTags TITLE=Song").unwrap();
    assert_eq!(uploads::infer_kind(temp.path()), "audio");
}

#[test]
fn overlapping_rich_quotes_require_offsets_and_accept_the_second_occurrence() {
    let source =
        json!({"blocks":[{"_":"pageBlockParagraph","text":{"_":"textPlain","text":"🙂ababa"}}]});
    assert!(
        rich_quote::select(&source, &json!({"quote_text":"aba"}))
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let (_, offset) =
        rich_quote::select(&source, &json!({"quote_text":"aba","quote_offset":4})).unwrap();
    assert_eq!(offset, 4);
}

#[test]
fn caption_credit_is_a_quote_source_and_shifts_following_utf16_offsets() {
    let source = json!({"blocks":[{"_":"pageBlockPhoto","photo_id":"1","caption":{"_":"pageCaption","text":plain("A"),"credit":plain("Alice")}},paragraph(plain("B"))]});
    let (credit, offset) =
        rich_quote::select(&source, &json!({"quote_text":"Alice","quote_offset":2})).unwrap();
    assert_eq!(offset, 2);
    assert_eq!(credit[0], plain("Alice"));
    assert_eq!(
        rich_quote::select(&source, &json!({"quote_text":"B","quote_offset":8}))
            .unwrap()
            .1,
        8
    );
}
