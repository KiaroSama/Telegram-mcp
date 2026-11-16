use super::support::*;
use serde_json::json;
#[tokio::test]
async fn location_answer_shares_only_the_explicit_coordinates() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeRequestGeoLocation"}))]),
        ),
        ("messages.sendMedia", updates()),
    ]);
    let mut args = keyboard_args();
    args["latitude"] = json!(48.5);
    args["longitude"] = json!(-123.25);
    run(&api, "answer_reply_button", args).await.unwrap();
    let req = &api.calls()[1].1;
    assert_eq!(
        req["media"],
        json!({"_":"inputMediaGeoPoint","geo_point":{"_":"inputGeoPoint","lat":48.5,"long":-123.25}})
    );
    assert_eq!(req["message"], "");
}
#[tokio::test]
async fn phone_answer_fetches_the_account_contact_and_ignores_caller_contact_fields() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeRequestPhone"}))]),
        ),
        (
            "users.getUsers",
            json!([{"_":"user","id":"7","phone":"00000000","first_name":"Synthetic","last_name":"Fixture"}]),
        ),
        ("messages.sendMedia", updates()),
    ]);
    let mut args = keyboard_args();
    args["phone"] = json!("caller-value-must-not-be-used");
    run(&api, "answer_reply_button", args).await.unwrap();
    let calls = api.calls();
    assert_eq!(calls[1].1["id"], json!([{"_":"inputUserSelf"}]));
    assert_eq!(
        calls[2].1["media"],
        json!({"_":"inputMediaContact","phone_number":"00000000","first_name":"Synthetic","last_name":"Fixture","vcard":""})
    );
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeRequestPhone"}))]),
        ),
        ("users.getUsers", json!([{"_":"user","id":"7","phone":""}])),
    ]);
    assert!(
        run(&api, "answer_reply_button", keyboard_args())
            .await
            .unwrap_err()
            .to_string()
            .contains("phone")
    );
    assert_eq!(api.calls().len(), 2);
}
#[tokio::test]
async fn requested_peers_bind_the_keyboard_message_button_id_and_selected_identities() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![
                message(99, "newer text"),
                keyboard(json!({"_":"buttonTypeRequestPeer","button_id":123,"max_quantity":2})),
            ]),
        ),
        ("messages.sendBotRequestedPeer", json!(true)),
    ])
    .identity(
        json!(8),
        json!({"_":"inputPeerUser","user_id":"8","access_hash":"18"}),
    )
    .identity(
        json!("channel"),
        json!({"_":"inputPeerChannel","channel_id":"9","access_hash":"19"}),
    );
    let mut args = keyboard_args();
    args["peers"] = json!([8, "channel"]);
    run(&api, "answer_reply_button", args).await.unwrap();
    let req = &api.calls()[1].1;
    assert_eq!(req["msg_id"], 88);
    assert_eq!(req["button_id"], 123);
    assert_eq!(
        req["requested_peers"],
        json!([{"_":"inputPeerUser","user_id":"8","access_hash":"18"},{"_":"inputPeerChannel","channel_id":"9","access_hash":"19"}])
    );
}
#[tokio::test]
async fn peer_request_quantity_bounds_refuse_without_resolving_selected_peers() {
    for selected in [json!([]), json!([1, 2]), json!("not-an-array")] {
        let api = Api::new(vec![(
            "messages.getHistory",
            messages(vec![keyboard(
                json!({"_":"buttonTypeRequestPeer","button_id":1}),
            )]),
        )]);
        let mut args = keyboard_args();
        args["peers"] = selected;
        assert!(run(&api, "answer_reply_button", args).await.is_err());
        assert_eq!(api.resolved.lock().unwrap().len(), 1);
        assert_eq!(api.calls().len(), 1);
    }
}
#[tokio::test]
async fn regular_and_quiz_poll_answers_preserve_the_requested_semantics_and_option_bytes() {
    for (quiz, correct) in [(false, None), (true, Some(1))] {
        let api = Api::new(vec![
            (
                "messages.getHistory",
                messages(vec![keyboard(
                    json!({"_":"buttonTypeRequestPoll","quiz":quiz}),
                )]),
            ),
            ("messages.sendMedia", updates()),
        ]);
        let mut args = keyboard_args();
        args["question"] = json!("پرسش 🙂");
        args["options"] = json!(["اول", "دوم"]);
        args["correct_option_index"] = json!(correct);
        run(&api, "answer_reply_button", args).await.unwrap();
        let calls = api.calls();
        let media = &calls[1].1["media"];
        assert_eq!(media["poll"]["quiz"], quiz);
        assert_eq!(media["poll"]["question"]["text"], "پرسش 🙂");
        assert_eq!(media["poll"]["answers"][0]["option"], json!([0]));
        assert_eq!(media["poll"]["answers"][1]["text"]["text"], "دوم");
        if quiz {
            assert_eq!(media["correct_answers"], json!([1]));
        } else {
            assert!(media.get("correct_answers").is_none());
        }
    }
}
#[tokio::test]
async fn invalid_poll_questions_answers_and_quiz_constraints_never_send_media() {
    for (kind, extra) in [
        (json!({"quiz":false}), json!({"question":""})),
        (json!({"quiz":false}), json!({"question":"🙂".repeat(151)})),
        (json!({"quiz":false}), json!({"options":"invalid"})),
        (json!({"quiz":false}), json!({"options":["only one"]})),
        (json!({"quiz":true}), json!({})),
        (json!({"quiz":false}), json!({"correct_option_index":0})),
        (json!({"quiz":true}), json!({"correct_option_index":2})),
        (json!({"quiz":false}), json!({"options":[1,"two"]})),
        (json!({"quiz":false}), json!({"options":["","two"]})),
        (
            json!({"quiz":false}),
            json!({"options":["🙂".repeat(51),"two"]}),
        ),
    ] {
        let mut kind = kind;
        kind["_"] = json!("buttonTypeRequestPoll");
        let api = Api::new(vec![(
            "messages.getHistory",
            messages(vec![keyboard(kind)]),
        )]);
        let mut args = keyboard_args();
        args["question"] = json!("question");
        args["options"] = json!(["one", "two"]);
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        assert!(run(&api, "answer_reply_button", args).await.is_err());
        assert_eq!(api.calls().len(), 1);
    }
}
#[tokio::test]
async fn simple_webview_uses_the_current_bot_and_returns_the_native_result() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(
                json!({"_":"buttonTypeSimpleWebView","url":"https://example.test/app"}),
            )]),
        ),
        (
            "messages.requestSimpleWebView",
            json!({"_":"simpleWebViewResultUrl","url":"https://example.test/session"}),
        ),
    ]);
    let result = run(&api, "answer_reply_button", keyboard_args())
        .await
        .unwrap();
    let req = &api.calls()[1].1;
    assert_eq!(req["platform"], "desktop");
    assert_eq!(req["bot"]["user_id"], "7");
    assert_eq!(req["url"], "https://example.test/app");
    assert_eq!(result["sent"]["url"], "https://example.test/session");
}
#[tokio::test]
async fn sensitive_buttons_require_the_dedicated_answer_route_and_unknown_kinds_send_nothing() {
    for kind in [
        "buttonTypeRequestPhone",
        "buttonTypeRequestPeer",
        "buttonTypeRequestPoll",
        "buttonTypeSimpleWebView",
        "buttonTypeRequestGeoLocation",
        "buttonTypeUnknown",
    ] {
        let api = Api::new(vec![(
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":kind}))]),
        )]);
        assert!(
            run(&api, "press_reply_button", keyboard_args())
                .await
                .is_err()
        );
        assert_eq!(api.calls().len(), 1);
    }
    let api = Api::new(vec![(
        "messages.getHistory",
        messages(vec![keyboard(json!({"_":"buttonTypeDefault"}))]),
    )]);
    assert!(
        run(&api, "answer_reply_button", keyboard_args())
            .await
            .is_err()
    );
    assert_eq!(api.calls().len(), 1);
}
#[tokio::test(start_paused = true)]
async fn waiting_for_a_bot_reply_ignores_outgoing_and_old_messages_without_marking_read() {
    let mut outgoing = message(100, "outgoing");
    outgoing["out"] = json!(true);
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeDefault"}))]),
        ),
        ("messages.sendMessage", updates()),
        (
            "messages.getHistory",
            messages(vec![
                outgoing,
                message(87, "older"),
                message(99, "incoming"),
            ]),
        ),
    ]);
    let mut args = keyboard_args();
    args["wait_seconds"] = json!(1.0);
    let result = run(&api, "press_reply_button", args).await.unwrap();
    assert_eq!(result["reply"]["id"], 99);
    assert_eq!(api.calls()[2].1["limit"], 10);
    assert_eq!(api.calls().len(), 3);
}
#[tokio::test(start_paused = true)]
async fn reply_wait_timeout_reports_sent_outcome_without_retrying_the_send() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeDefault"}))]),
        ),
        ("messages.sendMessage", updates()),
        ("messages.getHistory", messages(vec![])),
    ]);
    let mut args = keyboard_args();
    args["wait_seconds"] = json!(0.2);
    let result = run(&api, "press_reply_button", args).await.unwrap();
    assert_eq!(result["timed_out_waiting_for_reply"], true);
    assert_eq!(result["reply"], serde_json::Value::Null);
    assert_eq!(
        api.calls()
            .iter()
            .filter(|(name, _)| name == "messages.sendMessage")
            .count(),
        1
    );
}
#[tokio::test]
async fn invalid_wait_and_index_inputs_are_refused_before_telegram_history_is_read() {
    for patch in [
        json!({"wait_seconds":"invalid"}),
        json!({"wait_seconds":31}),
        json!({"wait_seconds":-1}),
        json!({"button_index":-1}),
    ] {
        let api = Api::new(vec![]);
        let mut args = keyboard_args();
        for (key, value) in patch.as_object().unwrap() {
            args[key] = value.clone();
        }
        assert!(run(&api, "press_reply_button", args).await.is_err());
        assert!(api.calls().is_empty());
    }
}
#[tokio::test]
async fn history_inspection_reports_the_active_reply_keyboard_without_fetching_by_id() {
    let api = Api::new(vec![(
        "messages.getHistory",
        messages(vec![
            message(99, "latest"),
            keyboard(json!({"_":"buttonTypeDefault"})),
        ]),
    )]);
    let result = run(&api, "inspect_buttons", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(result["message_id"], 99);
    assert_eq!(result["active_reply_keyboard"]["message_id"], 88);
    assert_eq!(api.calls()[0].1["limit"], 20);
    assert_eq!(api.calls().len(), 1);
    let api = Api::new(vec![("messages.getHistory", messages(vec![]))]);
    assert!(
        run(&api, "inspect_buttons", json!({"chat_id":7}))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn inline_query_handles_round_trip_lossless_ids_and_colons_in_result_identifiers() {
    let api = Api::new(vec![
        ("users.getUsers", json!([{"_":"user","id":"7","bot":true}])),
        (
            "messages.getInlineBotResults",
            json!({"_":"messages.botResults","query_id":"9223372036854775807","cache_time":0,"next_offset":"next","results":[{"_":"botInlineResult","id":"result:with:colons","type":"article","title":"answer"}],"users":[]}),
        ),
        ("messages.sendInlineBotResult", updates()),
    ]);
    let result = run(
        &api,
        "inline_query",
        json!({"bot":7,"query":"find","offset":"page-two","account":"Alice"}),
    )
    .await
    .unwrap();
    let handle = result["results"][0]["result_id"].as_str().unwrap();
    assert!(handle.starts_with("inline:alice:"));
    assert_eq!(api.calls()[1].1["offset"], "page-two");
    run(&api,"send_inline_result",json!({"chat_id":7,"account":"ALICE","result_id":handle,"topic_id":55,"reply_to_message_id":9})).await.unwrap();
    let req = &api.calls()[2].1;
    assert_eq!(req["query_id"], "9223372036854775807");
    assert_eq!(req["id"], "result:with:colons");
    assert_eq!(req["reply_to"]["reply_to_msg_id"], 9);
    assert_eq!(req["reply_to"]["top_msg_id"], 55);
}
#[tokio::test]
async fn inline_queries_refuse_non_bots_and_unparseable_query_handles() {
    let api = Api::new(vec![(
        "users.getUsers",
        json!([{"_":"user","id":"7","bot":false}]),
    )]);
    assert!(
        run(&api, "inline_query", json!({"bot":7,"query":"find"}))
            .await
            .is_err()
    );
    assert_eq!(api.calls().len(), 1);
    let future = telegram_mcp::tools::messaging::helpers::unix_time() + 3600;
    let api = Api::new(vec![]);
    for handle in [
        "not-a-handle".to_owned(),
        format!("inline:default:{future}:invalid-query-id:result"),
    ] {
        assert!(
            run(
                &api,
                "send_inline_result",
                json!({"chat_id":7,"result_id":handle})
            )
            .await
            .is_err()
        );
    }
    assert!(api.calls().is_empty());
}

#[tokio::test]
async fn a_reply_fetch_failure_preserves_the_completed_send_and_does_not_retry_it() {
    let api = Api::new(vec![
        (
            "messages.getHistory",
            messages(vec![keyboard(json!({"_":"buttonTypeDefault"}))]),
        ),
        ("messages.sendMessage", updates()),
    ]);
    api.error("messages.getHistory", "synthetic polling failure");
    let mut args = keyboard_args();
    args["wait_seconds"] = json!(1.0);
    let result = run(&api, "press_reply_button", args).await.unwrap();
    assert_eq!(result["sent"], updates());
    assert!(
        result["reply_fetch_warning"]
            .as_str()
            .unwrap()
            .contains("repeat")
    );
    assert_eq!(
        api.calls()
            .iter()
            .filter(|(name, _)| name == "messages.sendMessage")
            .count(),
        1
    );
}
