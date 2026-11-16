use super::*;

#[test]
fn short_history_pages_continue_until_the_server_returns_no_records() {
    if isolated("upstream::short_history_pages_continue_until_the_server_returns_no_records") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            short_page: true,
            ..Default::default()
        };
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "chat",
            &json!({"chat_id":"@test"}),
            &settings("short-pages"),
            receiver,
            "short-pages",
        )
        .await
        .unwrap();
        assert_eq!(result["messages"], 2);
        assert_eq!(result["complete"], true);
        let requests = api.requests.lock().unwrap();
        let offsets: Vec<_> = requests
            .iter()
            .filter(|(_, v)| v["query"]["_"] == "messages.getHistory")
            .map(|(_, v)| v["query"]["offset_id"].as_i64().unwrap())
            .collect();
        assert_eq!(offsets, vec![0, 9, 8]);
    });
}

#[test]
fn admin_log_partial_rich_messages_use_embedded_message_ids() {
    if isolated("upstream::admin_log_partial_rich_messages_use_embedded_message_ids") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            admin_partial: true,
            ..Default::default()
        };
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "recent_actions",
            &json!({"chat_id":"@test"}),
            &settings("admin-rich"),
            receiver,
            "admin-rich",
        )
        .await
        .unwrap();
        assert_eq!(result["events"], 1);
        assert!(
            api.requests
                .lock()
                .unwrap()
                .iter()
                .any(|(name, args)| name == "messages.getRichMessage" && args["id"] == 12)
        );
    });
}

#[test]
fn backwards_history_jumps_to_the_upper_date_bound_then_uses_exact_ids() {
    if isolated("upstream::backwards_history_jumps_to_the_upper_date_bound_then_uses_exact_ids") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut settings = settings("date-window");
        settings.from = 1699999999;
        settings.until = 1700000010;
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "chat",
            &json!({"chat_id":"@test"}),
            &settings,
            receiver,
            "date-window",
        )
        .await
        .unwrap();
        assert_eq!(result["messages"], 103);
        let requests = api.requests.lock().unwrap();
        let history: Vec<_> = requests
            .iter()
            .filter(|(_, request)| request["query"]["_"] == "messages.getHistory")
            .map(|(_, request)| &request["query"])
            .collect();
        assert_eq!(history[0]["offset_date"], 1700000010);
        assert_eq!(history[0]["offset_id"], 0);
        assert_eq!(history[1]["offset_id"], 101);
        assert!(
            history
                .iter()
                .skip(1)
                .all(|query| query["offset_date"] == 0)
        );
    });
}

#[test]
fn export_escapes_control_bytes_and_preserves_unicode_separators_without_invalid_json() {
    let text = "می‌کند\u{2028}paragraph\u{2029}\u{0000}\u{0001}\u{001f}😀";
    let message = json!({"_":"message","id":1,"date":1700000000,"message":text,"entities":[]});
    let rendered =
        export::render::json_message(&message, &std::collections::BTreeMap::new()).unwrap();
    let encoded = serde_json::to_string(&rendered).unwrap();
    assert!(encoded.contains("\\u0000"));
    assert!(encoded.contains("\\u0001"));
    assert!(encoded.contains("\\u001f"));
    let decoded: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded["text"], text);
    let html = export::render::html_escape(text);
    assert!(html.contains("می‌کند<br>paragraph<br>"));
    assert!(html.contains("&#x00;&#x01;&#x1F;😀"));
}

#[test]
fn export_json_never_emits_nonfinite_ratings_as_invalid_numbers() {
    let message = json!({"_":"message","id":1,"date":1700000000,"message":"x","stars_rating":f64::INFINITY,"rating":f64::NAN});
    let rendered =
        export::render::json_message(&message, &std::collections::BTreeMap::new()).unwrap();
    let encoded = serde_json::to_string(&rendered).unwrap();
    assert!(!encoded.contains("Infinity"));
    assert!(!encoded.contains("NaN"));
    assert!(serde_json::from_str::<Value>(&encoded).is_ok());
    assert!(rendered["telegram_tl"]["stars_rating"].is_null());
}
