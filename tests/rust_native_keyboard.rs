//! Wire fixtures originate in the pinned grammers serializer, not hand-written
//! JSON assumptions about an older Telegram layer's button constructors.
#[path = "common/messaging.rs"]
mod support;
use grammers_tl_types::{self as tl, Deserializable, Serializable};
use serde_json::{Value, json};
use support::*;
use telegram_mcp::tl::Schema;

fn native(markup: tl::enums::ReplyMarkup) -> Value {
    let bytes = markup.to_bytes();
    assert_eq!(tl::enums::ReplyMarkup::from_bytes(&bytes).unwrap(), markup);
    Schema::global().decode("ReplyMarkup", &bytes).unwrap()
}
fn inline(buttons: Vec<tl::types::KeyboardInlineButton>) -> Value {
    native(
        tl::types::ReplyInlineMarkup {
            force_reply: false,
            rows: vec![
                tl::types::KeyboardInlineButtonRow {
                    buttons: buttons.into_iter().map(Into::into).collect(),
                }
                .into(),
            ],
        }
        .into(),
    )
}
#[tokio::test]
async fn generated_callback_wire_preserves_nested_payload_password_flags_and_style() {
    let style: tl::enums::KeyboardButtonStyle = tl::types::KeyboardButtonStyle {
        bg_primary: true,
        bg_danger: false,
        bg_success: false,
        icon: Some(123),
    }
    .into();
    let markup = inline(vec![
        tl::types::KeyboardInlineButton {
            style: Some(style),
            text: "press".into(),
            r#type: tl::types::InlineButtonTypeCallback {
                requires_password: false,
                data: vec![0, 128, 255],
            }
            .into(),
        },
        tl::types::KeyboardInlineButton {
            style: None,
            text: "password".into(),
            r#type: tl::types::InlineButtonTypeCallback {
                requires_password: true,
                data: vec![9],
            }
            .into(),
        },
    ]);
    let mut message = message(10, "");
    message["reply_markup"] = markup;
    let api = Wire::new(vec![
        messages(vec![message.clone()]),
        json!([{"_":"document","id":"123","mime_type":"image/webp","attributes":[{"_":"documentAttributeCustomEmoji","alt":"icon"}]}]),
        messages(vec![message]),
        json!({"message":"native callback accepted"}),
    ]);
    let inspected = run(
        &api,
        "inspect_buttons",
        json!({"chat_id":7,"message_id":10}),
    )
    .await
    .unwrap();
    assert_eq!(inspected["buttons"][0]["kind"], "inlineButtonTypeCallback");
    assert_eq!(
        inspected["buttons"][0]["type"]["data"],
        json!([0, 128, 255])
    );
    assert_eq!(inspected["buttons"][0]["style"]["bg_primary"], true);
    assert_eq!(inspected["buttons"][0]["style"]["icon"], "123");
    assert_eq!(inspected["buttons"][0]["icon"]["alt"], "icon");
    assert_eq!(inspected["buttons"][1]["type"]["requires_password"], true);
    assert!(inspected["buttons"][1].get("press_token").is_none());
    let result=run(&api,"click_button",json!({"chat_id":7,"message_id":10,"button_index":0,"expect_text":"press","press_token":inspected["buttons"][0]["press_token"]})).await.unwrap();
    assert_eq!(result["message"], "native callback accepted");
    assert_eq!(api.calls()[3].1["data"], json!([0, 128, 255]));
}
#[tokio::test]
async fn generated_url_and_copy_buttons_keep_their_native_fields_without_callback_tokens() {
    let markup = inline(vec![
        tl::types::KeyboardInlineButton {
            style: None,
            text: "open".into(),
            r#type: tl::types::InlineButtonTypeUrl {
                url: "https://example.test/native".into(),
            }
            .into(),
        },
        tl::types::KeyboardInlineButton {
            style: None,
            text: "copy".into(),
            r#type: tl::types::InlineButtonTypeCopy {
                copy_text: "سلام 🙂".into(),
            }
            .into(),
        },
    ]);
    let mut message = message(10, "");
    message["reply_markup"] = markup;
    let api = Wire::new(vec![messages(vec![message])]);
    let inspected = run(
        &api,
        "inspect_buttons",
        json!({"chat_id":7,"message_id":10}),
    )
    .await
    .unwrap();
    assert_eq!(
        inspected["buttons"][0]["url"],
        "https://example.test/native"
    );
    assert_eq!(inspected["buttons"][1]["copy_text"], "سلام 🙂");
    assert!(
        inspected["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .all(|button| button.get("press_token").is_none())
    );
}
#[tokio::test]
async fn generated_reply_button_wire_keeps_the_nested_type_and_exact_unicode_label() {
    let markup = native(
        tl::types::ReplyKeyboardMarkup {
            resize: true,
            single_use: true,
            selective: false,
            persistent: false,
            force_reply: false,
            placeholder: Some("choose".into()),
            rows: vec![
                tl::types::KeyboardButtonRow {
                    buttons: vec![
                        tl::types::KeyboardButton {
                            style: None,
                            text: "سلام 🙂".into(),
                            r#type: tl::enums::ButtonType::Default,
                        }
                        .into(),
                    ],
                }
                .into(),
            ],
        }
        .into(),
    );
    assert_eq!(
        markup["rows"][0]["buttons"][0]["type"]["_"],
        "buttonTypeDefault"
    );
    let mut message = message(10, "");
    message["reply_markup"] = markup;
    let api = Wire::new(vec![messages(vec![message]), json!({})]);
    run(
        &api,
        "press_reply_button",
        json!({"chat_id":7,"button_index":0,"expect_text":"سلام 🙂","wait_seconds":0}),
    )
    .await
    .unwrap();
    assert_eq!(api.calls()[1].0, "messages.sendMessage");
    assert_eq!(api.calls()[1].1["message"], "سلام 🙂");
}
