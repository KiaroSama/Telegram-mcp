//! Independent wire evidence against grammers' generated serializers.
use grammers_tl_types::{self as tl, Serializable};
use serde_json::json;
use telegram_mcp::{args, backend::parse_text, tl::Schema};

#[test]
fn native_schema_layer_matches_transport_layer() {
    assert_eq!(telegram_mcp::tl::LAYER, tl::LAYER);
}
#[test]
fn history_request_bytes_match_grammers() {
    let typed = tl::functions::messages::GetHistory {
        peer: tl::enums::InputPeer::PeerSelf,
        offset_id: 7,
        offset_date: 123,
        add_offset: -5,
        limit: 20,
        max_id: 99,
        min_id: 1,
        hash: i64::MAX,
    };
    let params = json!({"peer":{"_":"inputPeerSelf"},"offset_id":7,"offset_date":123,"add_offset":-5,"limit":20,"max_id":99,"min_id":1,"hash":i64::MAX.to_string()});
    assert_eq!(
        Schema::global()
            .encode_call("messages.getHistory", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn telegram_booleans_match_grammers() {
    for value in [false, true] {
        assert_eq!(
            Schema::global().decode("Bool", &value.to_bytes()).unwrap(),
            json!(value)
        );
    }
}
#[test]
fn mandatory_bool_fields_match_layer233_grammers() {
    let typed = tl::functions::channels::ToggleForum {
        channel: tl::enums::InputChannel::Channel(tl::types::InputChannel {
            channel_id: 123,
            access_hash: -456,
        }),
        enabled: true,
        tabs: false,
        sidebar: true,
    };
    let params = json!({"channel":{"_":"inputChannel","channel_id":"123","access_hash":"-456"},"enabled":true,"tabs":false,"sidebar":true});
    assert_eq!(
        Schema::global()
            .encode_call("channels.toggleForum", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn optional_false_is_serialized_with_its_presence_flag() {
    let typed = tl::functions::messages::EditForumTopic {
        peer: tl::enums::InputPeer::PeerSelf,
        topic_id: 7,
        title: None,
        icon_emoji_id: None,
        closed: Some(false),
        hidden: None,
    };
    let params = json!({"peer":{"_":"inputPeerSelf"},"topic_id":7,"closed":false});
    assert_eq!(
        Schema::global()
            .encode_call("messages.editForumTopic", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn omitted_optional_bool_keeps_flag_absent() {
    let typed = tl::functions::messages::EditForumTopic {
        peer: tl::enums::InputPeer::PeerSelf,
        topic_id: 7,
        title: None,
        icon_emoji_id: None,
        closed: None,
        hidden: None,
    };
    let params = json!({"peer":{"_":"inputPeerSelf"},"topic_id":7,"closed":null});
    assert_eq!(
        Schema::global()
            .encode_call("messages.editForumTopic", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn poll_vote_bytes_and_utf8_offset_match_grammers() {
    let typed = tl::functions::messages::GetPollVotes {
        peer: tl::enums::InputPeer::PeerSelf,
        id: 7,
        option: Some(vec![0, 241, 255]),
        offset: Some("صفحه🙂".into()),
        limit: 20,
    };
    let params = json!({"peer":{"_":"inputPeerSelf"},"id":7,"option":[0,241,255],"offset":"صفحه🙂","limit":20});
    assert_eq!(
        Schema::global()
            .encode_call("messages.getPollVotes", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn binary_payload_matches_grammers() {
    let typed = tl::functions::auth::ImportAuthorization {
        id: i64::MIN,
        bytes: vec![0, 1, 2, 255],
    };
    let params = json!({"id":i64::MIN.to_string(),"bytes":[0,1,2,255]});
    assert_eq!(
        Schema::global()
            .encode_call("auth.importAuthorization", &params)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn base64_payload_encodes_to_identical_wire_bytes() {
    let typed = tl::functions::auth::ImportAuthorization {
        id: 1,
        bytes: vec![0, 1, 2, 255],
    };
    assert_eq!(
        Schema::global()
            .encode_call(
                "auth.importAuthorization",
                &json!({"id":"1","bytes":"AAEC/w=="})
            )
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn short_utf8_strings_round_trip_without_replacement() {
    let typed = tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
        text: "سلام 🙂 café 漢字".into(),
        entities: vec![],
    });
    let encoded = Schema::global()
        .encode_object(&json!({"_":"textWithEntities","text":"سلام 🙂 café 漢字","entities":[]}))
        .unwrap();
    assert_eq!(encoded, typed.to_bytes());
    assert_eq!(
        Schema::global()
            .decode("TextWithEntities", &encoded)
            .unwrap()["text"],
        "سلام 🙂 café 漢字"
    );
}
#[test]
fn long_utf8_strings_use_24bit_length_prefix() {
    let text = "سلام🙂".repeat(80);
    let typed = tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
        text: text.clone(),
        entities: vec![],
    });
    let encoded = Schema::global()
        .encode_object(&json!({"_":"textWithEntities","text":text,"entities":[]}))
        .unwrap();
    assert_eq!(encoded[4], 254);
    assert_eq!(encoded, typed.to_bytes());
    assert_eq!(
        Schema::global()
            .decode("TextWithEntities", &encoded)
            .unwrap()["text"],
        text
    );
}
#[test]
fn integer_vectors_decode_like_grammers_values() {
    let values = vec![-7i32, 0, i32::MAX];
    assert_eq!(
        Schema::global()
            .decode("Vector<int>", &values.to_bytes())
            .unwrap(),
        json!([-7, 0, i32::MAX])
    );
}
#[test]
fn bytes_decode_as_exact_unsigned_values() {
    assert_eq!(
        Schema::global()
            .decode("bytes", &vec![0u8, 128, 255].to_bytes())
            .unwrap(),
        json!([0, 128, 255])
    );
}
#[test]
fn boolean_vectors_preserve_false_entries() {
    assert_eq!(
        Schema::global()
            .decode("Vector<Bool>", &vec![true, false].to_bytes())
            .unwrap(),
        json!([true, false])
    );
}
#[test]
fn max_long_is_returned_as_lossless_decimal_string() {
    assert_eq!(
        Schema::global()
            .decode("long", &i64::MAX.to_bytes())
            .unwrap(),
        json!("9223372036854775807")
    );
}
#[test]
fn min_long_is_returned_as_lossless_decimal_string() {
    assert_eq!(
        Schema::global()
            .decode("long", &i64::MIN.to_bytes())
            .unwrap(),
        json!("-9223372036854775808")
    );
}
#[test]
fn shared_flag_fields_must_be_supplied_together() {
    let p = json!({"peer":{"_":"inputPeerSelf"},"to_lang":"fa"});
    assert!(
        Schema::global()
            .encode_call("messages.translateText", &p)
            .is_err()
    );
    let p = json!({"id":[1],"to_lang":"fa"});
    assert!(
        Schema::global()
            .encode_call("messages.translateText", &p)
            .is_err()
    );
}
#[test]
fn shared_flag_translation_matches_grammers() {
    let typed = tl::functions::messages::TranslateText {
        peer: Some(tl::enums::InputPeer::PeerSelf),
        id: Some(vec![1, 2]),
        text: None,
        to_lang: "fa".into(),
        tone: None,
    };
    let p = json!({"peer":{"_":"inputPeerSelf"},"id":[1,2],"to_lang":"fa"});
    assert_eq!(
        Schema::global()
            .encode_call("messages.translateText", &p)
            .unwrap(),
        typed.to_bytes()
    );
}
#[test]
fn unknown_method_is_rejected() {
    assert!(
        Schema::global()
            .encode_call("messages.doesNotExist", &json!({}))
            .is_err()
    );
}
#[test]
fn constructors_cannot_be_invoked_as_functions() {
    assert!(
        Schema::global()
            .encode_call("inputPeerSelf", &json!({}))
            .is_err()
    );
}
#[test]
fn missing_mandatory_parameter_is_named() {
    let e = Schema::global()
        .encode_call(
            "messages.getHistory",
            &json!({"peer":{"_":"inputPeerSelf"}}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("offset_id"));
}
#[test]
fn unknown_parameter_is_not_silently_dropped() {
    assert!(
        Schema::global()
            .encode_call("messages.getDialogFilters", &json!({"typo":true}))
            .is_err()
    );
}
#[test]
fn wrong_constructor_type_is_rejected() {
    assert!(
        Schema::global()
            .encode_call(
                "channels.toggleForum",
                &json!({"channel":{"_":"inputPeerSelf"},"enabled":true,"tabs":true,"sidebar":false})
            )
            .is_err()
    );
}
#[test]
fn int_overflow_is_rejected_before_serialization() {
    assert!(
        Schema::global()
            .encode_call(
                "messages.getPollVotes",
                &json!({"peer":{"_":"inputPeerSelf"},"id":2147483648i64,"limit":1})
            )
            .is_err()
    );
}
#[test]
fn fractional_integer_is_refused() {
    assert!(
        Schema::global()
            .encode_call("auth.importAuthorization", &json!({"id":1.5,"bytes":[]}))
            .is_err()
    );
}
#[test]
fn malformed_byte_values_are_refused() {
    for value in [json!([256]), json!([-1]), json!([true]), json!([1.5])] {
        assert!(
            Schema::global()
                .encode_call("auth.importAuthorization", &json!({"id":"1","bytes":value}))
                .is_err()
        );
    }
}
#[test]
fn malformed_base64_is_refused() {
    assert!(
        Schema::global()
            .encode_call("auth.importAuthorization", &json!({"id":"1","bytes":"###"}))
            .is_err()
    );
}
#[test]
fn malformed_utf8_is_refused_without_lossy_decoding() {
    assert!(
        Schema::global()
            .decode("string", &[2, 0xc3, 0x28, 0])
            .is_err()
    );
}
#[test]
fn truncated_integer_is_refused() {
    assert!(Schema::global().decode("int", &[1, 2, 3]).is_err());
}
#[test]
fn truncated_long_is_refused() {
    assert!(
        Schema::global()
            .decode("long", &[1, 2, 3, 4, 5, 6, 7])
            .is_err()
    );
}
#[test]
fn truncated_string_is_refused() {
    assert!(Schema::global().decode("string", &[3, b'a']).is_err());
}
#[test]
fn truncated_flags_are_refused() {
    let bytes = Schema::global()
        .definition("messageEntityBlockquote")
        .unwrap()
        .id
        .to_le_bytes();
    assert!(Schema::global().decode("MessageEntity", &bytes).is_err());
}
#[test]
fn unknown_constructor_id_is_refused() {
    assert!(
        Schema::global()
            .decode("InputPeer", &0xdeadbeefu32.to_le_bytes())
            .is_err()
    );
}
#[test]
fn wrong_known_constructor_is_refused() {
    let bytes = Schema::global()
        .encode_object(&json!({"_":"inputPeerSelf"}))
        .unwrap();
    assert!(Schema::global().decode("InputChannel", &bytes).is_err());
}
#[test]
fn unknown_boolean_constructor_is_refused() {
    assert!(
        Schema::global()
            .decode("Bool", &0xdeadbeefu32.to_le_bytes())
            .is_err()
    );
}
#[test]
fn trailing_bytes_after_result_are_refused() {
    let mut bytes = 1i32.to_bytes();
    bytes.extend(2i32.to_bytes());
    assert!(Schema::global().decode("int", &bytes).is_err());
}
#[test]
fn nonzero_string_padding_is_refused() {
    assert!(Schema::global().decode("string", &[1, b'a', 0, 1]).is_err());
}
#[test]
fn reserved_bytes_length_marker_is_refused() {
    assert!(Schema::global().decode("bytes", &[255, 0, 0, 0]).is_err());
}
#[test]
fn oversized_vector_is_refused_before_allocation() {
    let mut bytes = 0x1cb5c415u32.to_le_bytes().to_vec();
    bytes.extend(100001u32.to_le_bytes());
    assert!(Schema::global().decode("Vector<int>", &bytes).is_err());
}
#[test]
fn wrong_vector_constructor_is_refused() {
    let mut bytes = 0xdeadbeefu32.to_le_bytes().to_vec();
    bytes.extend(0u32.to_le_bytes());
    assert!(Schema::global().decode("Vector<int>", &bytes).is_err());
}
#[test]
fn nan_and_infinity_responses_are_refused() {
    for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Schema::global().decode("double", &n.to_le_bytes()).is_err());
    }
}
#[test]
fn excessive_object_depth_is_refused() {
    let mut media = json!({"_":"inputMediaEmpty"});
    for _ in 0..70 {
        media = json!({"_":"inputMediaPaidMedia","stars_amount":"1","extended_media":[media]});
    }
    let args = json!({"peer":{"_":"inputPeerSelf"},"media":media,"message":"","random_id":"1"});
    assert!(
        Schema::global()
            .encode_call("messages.sendMedia", &args)
            .is_err()
    );
}
#[test]
fn excessive_response_depth_is_refused() {
    let mut bytes = Vec::new();
    let concat = Schema::global().definition("textConcat").unwrap().id;
    for _ in 0..70 {
        bytes.extend(concat.to_le_bytes());
        bytes.extend(0x1cb5c415u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
    }
    bytes.extend(
        Schema::global()
            .definition("textEmpty")
            .unwrap()
            .id
            .to_le_bytes(),
    );
    assert!(Schema::global().decode("RichText", &bytes).is_err());
}
#[test]
fn markdown_entities_keep_utf16_offsets_after_non_bmp_text() {
    let (text, entities) = parse_text("🙂 **سلام**", Some("md")).unwrap();
    assert_eq!(text, "🙂 سلام");
    assert_eq!(entities[0]["_"], "messageEntityBold");
    assert_eq!(entities[0]["offset"], 3);
    assert_eq!(entities[0]["length"], 4);
}
#[test]
fn html_entities_keep_non_ascii_text() {
    let (text, entities) = parse_text("<b>سلام</b> 🙂", Some("html")).unwrap();
    assert_eq!(text, "سلام 🙂");
    assert_eq!(entities[0]["length"], 4);
}
#[test]
fn plain_parse_mode_preserves_markup_verbatim() {
    let (text, entities) = parse_text("**سلام**", None).unwrap();
    assert_eq!(text, "**سلام**");
    assert!(entities.is_empty());
}
#[test]
fn unknown_parse_mode_is_not_silently_ignored() {
    assert!(parse_text("hello", Some("unknown")).is_err());
}
#[test]
fn argument_ids_reject_booleans() {
    assert!(args::integer(&json!({"id":true}), "id", 0).is_err());
}
#[test]
fn argument_limit_caps_large_requests() {
    assert_eq!(
        args::limit(&json!({"limit":1000}), "limit", 50, 200).unwrap(),
        200
    );
}
#[test]
fn argument_limit_rejects_nonpositive_requests() {
    for n in [-1, 0] {
        assert!(args::limit(&json!({"limit":n}), "limit", 50, 200).is_err());
    }
}
#[test]
fn argument_text_is_not_coerced_from_json_numbers() {
    assert!(args::text(&json!({"text":42}), "text").is_err());
}
#[test]
fn argument_id_string_keeps_full_64bit_value() {
    assert_eq!(
        args::integer(&json!({"id":"9223372036854775807"}), "id", 0).unwrap(),
        i64::MAX
    );
}
#[test]
fn required_arguments_refuse_null() {
    assert!(args::required(&json!({"id":null}), "id").is_err());
}
#[test]
fn random_message_ids_are_signed_decimal_strings() {
    let id = args::random_id();
    assert!(id.as_str().unwrap().parse::<i64>().is_ok());
}
#[test]
fn object_builder_preserves_fields_and_assigns_constructor() {
    assert_eq!(
        args::object("inputPeerUser", json!({"user_id":"7","access_hash":"9"})),
        json!({"_":"inputPeerUser","user_id":"7","access_hash":"9"})
    );
}
#[test]
fn flag_only_boolean_uses_no_boolean_constructor() {
    for value in [false, true] {
        let object = json!({"_":"messageEntityBlockquote","collapsed":value,"offset":0,"length":3});
        let bytes = Schema::global().encode_object(&object).unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            u32::from(value)
        );
        assert_eq!(
            Schema::global().decode("MessageEntity", &bytes).unwrap()["collapsed"],
            value
        );
    }
}
#[test]
fn encoder_refuses_vectors_over_the_item_budget() {
    let p = json!({"peer":{"_":"inputPeerSelf"},"msg_id":1,"options":vec![json!([]);100001]});
    assert!(
        Schema::global()
            .encode_call("messages.sendVote", &p)
            .is_err()
    );
}
#[test]
fn fixed_width_int128_is_not_length_prefixed() {
    let bytes = vec![0x55; 16];
    assert_eq!(
        Schema::global().decode("int128", &bytes).unwrap(),
        json!(bytes)
    );
}
#[test]
fn fixed_width_int128_rejects_wrong_length() {
    assert!(Schema::global().decode("int128", &[0; 15]).is_err());
    assert!(Schema::global().decode("int128", &[0; 17]).is_err());
}
