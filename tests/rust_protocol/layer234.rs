use super::{Schema, Serializable, json, tl, tl_fixture};
use telegram_mcp::export::render::{html_message, json_message};

#[test]
fn forum_topic_filters_match_layer234_wire_bytes() {
    for closed in [None, Some(false), Some(true)] {
        let typed = tl::functions::messages::GetForumTopics {
            peer: tl::enums::InputPeer::PeerSelf,
            q: None,
            icon_emoji_id: Some(i64::MAX),
            closed,
            offset_date: 0,
            offset_id: 0,
            offset_topic: 0,
            limit: 5,
        };
        let params = json!({"peer":{"_":"inputPeerSelf"},"icon_emoji_id":i64::MAX.to_string(),"closed":closed,"offset_date":0,"offset_id":0,"offset_topic":0,"limit":5});
        assert_eq!(
            Schema::global()
                .encode_call("messages.getForumTopics", &params)
                .unwrap(),
            typed.to_bytes()
        );
    }
}

#[test]
fn optional_forum_icon_vector_decodes_null_empty_and_lossless_ids() {
    for value in [json!(null), json!([]), json!([i64::MAX.to_string()])] {
        let result = tl_fixture::decode_object(
            "messages.ForumTopics",
            &json!({"_":"messages.forumTopics","top_icons":value,"count":0,"topics":[],"messages":[],"chats":[],"users":[],"pts":0}),
        );
        assert_eq!(result["top_icons"], value);
    }
}

#[test]
fn layer234_actions_survive_wire_decode_and_both_export_formats() {
    let schema = Schema::global();
    let invite = tl::enums::MessageAction::BotInvite(tl::types::MessageActionBotInvite {
        inviter_id: i64::MAX,
    });
    let decoded = schema.decode("MessageAction", &invite.to_bytes()).unwrap();
    assert_eq!(
        decoded,
        json!({"_":"messageActionBotInvite","inviter_id":i64::MAX.to_string()})
    );
    let gram = tl_fixture::decode_object(
        "MessageAction",
        &json!({"_":"messageActionGramTransfer","amount":"42","peer_address":"synthetic-address","transaction_id":"synthetic-reference","onramp_provider_name":"Provider <&>"}),
    );
    assert_eq!(gram["onramp_provider_name"], "Provider <&>");
    assert_eq!(gram["comment"], serde_json::Value::Null);
    let gift = tl_fixture::decode_object(
        "MessageAction",
        &json!({
            "_":"messageActionStarGiftOnchainOffer","offer_id":"11","amount":"42","fee_bps":100,
            "expires_at":1700000000,"offer_address":"synthetic-offer","buyer":true,
            "gift":{"_":"starGift","id":"4","sticker":{"_":"documentEmpty","id":"6"},"stars":"5","convert_stars":"3"}
        }),
    );
    assert_eq!(gift["offer_id"], "11");
    assert_eq!(gift["buyer"], true);
    assert!(gift["buyer_refund"].is_null());
    assert_eq!(
        schema
            .definition("messageActionStarGiftOnchainOffer")
            .unwrap()
            .id,
        0x444a6568
    );
    for action in [decoded, gram, gift] {
        let message = json!({"_":"messageService","id":7,"date":1700000000,"action":action});
        let rendered = json_message(&message, &Default::default()).unwrap();
        assert_eq!(rendered["action"], action);
        let html = html_message(&message, &Default::default()).unwrap();
        assert!(html.contains(action["_"].as_str().unwrap()));
        assert!(!html.contains("Provider <&>"));
    }
}
