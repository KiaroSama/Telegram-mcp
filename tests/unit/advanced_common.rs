use super::*;

struct Api {
    response: Option<Value>,
    pending: bool,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
    key: Option<String>,
    gate: Option<(
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Semaphore>,
    )>,
}
impl Api {
    fn returning(response: Option<Value>) -> Self {
        Self {
            response,
            pending: false,
            calls: std::sync::Mutex::new(vec![]),
            key: None,
            gate: None,
        }
    }
}
#[async_trait::async_trait]
impl Telegram for Api {
    fn cache_key(&self) -> Option<String> {
        self.key.clone()
    }
    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(method, &args)?;
        self.calls.lock().unwrap().push((method.into(), args));
        if let Some((entered, release)) = &self.gate {
            entered.notify_one();
            release.acquire().await.unwrap().forget();
        }
        if self.pending {
            return std::future::pending().await;
        }
        self.response
            .clone()
            .ok_or_else(|| anyhow::anyhow!("RPC unavailable"))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        anyhow::bail!("Unexpected peer lookup")
    }
}

#[tokio::test]
async fn app_limit_refresh_serializes_one_account_without_blocking_other_accounts() {
    use std::sync::Arc;
    let mut warm = Api::returning(Some(
        json!({"config":{"value":[{"key":"quota","value":{"value":12}}]}}),
    ));
    warm.key = Some(format!("warm-limits-{}", rand::random::<u64>()));
    assert_eq!(app_limits(&warm).await["quota"], 12);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut slow = Api::returning(Some(
        json!({"config":{"value":[{"key":"quota","value":{"value":20}}]}}),
    ));
    slow.key = Some(format!("slow-limits-{}", rand::random::<u64>()));
    slow.gate = Some((Arc::clone(&entered), Arc::clone(&release)));
    let slow = Arc::new(slow);
    let first = Arc::clone(&slow);
    let one = tokio::spawn(async move { app_limits(first.as_ref()).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), app_limits(&warm))
            .await
            .unwrap()["quota"],
        12
    );
    let second = Arc::clone(&slow);
    let (started, running) = tokio::sync::oneshot::channel();
    let two = tokio::spawn(async move {
        started.send(()).unwrap();
        app_limits(second.as_ref()).await
    });
    running.await.unwrap();
    assert_eq!(slow.calls.lock().unwrap().len(), 1);
    release.add_permits(2);
    assert_eq!(one.await.unwrap()["quota"], 20);
    assert_eq!(two.await.unwrap()["quota"], 20);
    assert_eq!(slow.calls.lock().unwrap().len(), 1);
    assert_eq!(warm.calls.lock().unwrap().len(), 1);
}

#[test]
fn date_time_arguments_preserve_offsets_and_accept_legacy_utc_without_fraction_loss() {
    assert_eq!(
        timestamp(&json!({"when":"1970-01-01 00:00:01"}), "when").unwrap(),
        1
    );
    assert_eq!(
        timestamp(&json!({"when":"1970-01-01T03:30:00+03:30"}), "when").unwrap(),
        0
    );
    assert_eq!(
        timestamp(&json!({"when":"1970-01-01T00:00:01"}), "when").unwrap(),
        1
    );
    assert_eq!(
        timestamp(&json!({"when":"-9223372036854775808"}), "when").unwrap(),
        i64::MIN
    );
    assert!(timestamp(&json!({"when":"2026-02-30T00:00:00"}), "when").is_err());
    assert!(timestamp(&json!({"when":false}), "when").is_err());
}

#[test]
fn telegram_invite_parsing_accepts_official_aliases_and_rejects_ambiguous_tg_slugs() {
    for raw in [
        "t.me/addlist/folder_token",
        "telegram.me/addlist/folder_token/",
        "tg://addlist?slug=folder_token",
    ] {
        assert_eq!(slug(raw, "addlist").unwrap(), "folder_token");
    }
    assert_eq!(
        slug("tg://message?slug=business_token", "message").unwrap(),
        "business_token"
    );
    for raw in [
        "tg://addlist?slug=one&slug=two",
        "tg://addlist?other=value",
        "tg://message?slug=one",
        "https://t.me.evil/addlist/one",
        "https://person@t.me/addlist/one",
    ] {
        assert!(slug(raw, "addlist").is_err(), "{raw}");
    }
}

#[test]
fn optional_fields_keep_explicit_false_and_zero_without_turning_null_into_a_clear() {
    let mut output = json!({"keep":1});
    copy_optional(
        &json!({"enabled":false,"count":0,"absent":null}),
        &mut output,
        &["enabled", "count", "absent"],
    );
    assert_eq!(output, json!({"keep":1,"enabled":false,"count":0}));
    assert_eq!(
        string(&json!({"text":null}), "text", "default").unwrap(),
        "default"
    );
    assert!(string(&json!({"text":true}), "text", "default").is_err());
}

#[test]
fn reaction_selection_distinguishes_unicode_custom_ids_and_explicit_removal() {
    assert_eq!(
        reaction(&json!({"emoji":"❤\u{fe0f}"}), false).unwrap()["emoticon"],
        "❤\u{fe0f}"
    );
    assert_eq!(
        reaction(&json!({"custom_emoji_id":"9007199254740993"}), false).unwrap()["document_id"],
        "9007199254740993"
    );
    assert_eq!(reaction(&json!({}), true).unwrap()["_"], "reactionEmpty");
    for input in [
        json!({}),
        json!({"emoji":"😀","custom_emoji_id":42}),
        json!({"custom_emoji_id":0}),
    ] {
        assert!(reaction(&input, false).is_err());
    }
}

#[test]
fn peer_adapters_require_the_correct_kind_and_preserve_self_identity() {
    assert_eq!(
        user(&json!({"_":"inputPeerSelf"})).unwrap(),
        json!({"_":"inputUserSelf"})
    );
    assert!(user(&json!({"_":"inputPeerChannel","channel_id":7,"access_hash":"1"})).is_err());
    assert!(channel(&json!({"_":"inputPeerUser","user_id":7,"access_hash":"1"})).is_err());
    let input =
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"-9223372036854775808"});
    assert_eq!(
        channel(&input).unwrap()["access_hash"],
        "-9223372036854775808"
    );
}

#[test]
fn uploaded_attachment_references_preserve_file_bytes_and_reject_empty_media() {
    let photo = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"42","access_hash":"-99","file_reference":[0,255]}});
    let converted = as_input_media(&photo).unwrap();
    assert_eq!(converted["id"]["file_reference"], json!([0, 255]));
    assert_eq!(converted["id"]["access_hash"], "-99");
    let doc = json!({"_":"messageMediaDocument","document":{"_":"document","id":"43","access_hash":"-98"}});
    assert_eq!(
        as_input_media(&doc).unwrap()["id"]["file_reference"],
        json!([])
    );
    assert!(document(&json!({"_":"documentEmpty","id":43})).is_err());
    assert!(as_input_media(&json!({"_":"messageMediaUnsupported"})).is_err());
}

#[tokio::test]
async fn message_lookup_checks_identity_and_uses_private_chat_rpc_without_read_receipts() {
    let api = Api::returning(Some(
        json!({"messages":[{"id":5,"message":"wrong","peer_id":{"_":"peerUser","user_id":"42"}},{"id":7,"message":"selected","peer_id":{"_":"peerUser","user_id":"42"}}]}),
    ));
    let peer = json!({"_":"inputPeerUser","user_id":"42","access_hash":"99"});
    let selected = message(&api, &peer, 7).await.unwrap();
    assert_eq!(selected["message"], "selected");
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "messages.getMessages");
    assert_eq!(calls[0].1["id"][0]["id"], 7);
}

#[tokio::test]
async fn missing_message_or_invalid_id_never_selects_an_unrelated_message() {
    let api = Api::returning(Some(json!({"messages":[{"id":5}]})));
    let peer = json!({"_":"inputPeerUser","user_id":"42","access_hash":"99"});
    assert!(
        message(&api, &peer, 7)
            .await
            .unwrap_err()
            .to_string()
            .contains("not found")
    );
    for id in [0, -1, i32::MAX as i64 + 1] {
        assert!(message(&api, &peer, id).await.is_err());
    }
    assert_eq!(api.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn app_config_failure_does_not_invent_telegram_quotas() {
    let api = Api::returning(None);
    assert_eq!(app_limits(&api).await, json!({}));
    assert_eq!(api.calls.lock().unwrap().len(), 1);
    let api = Api::returning(Some(
        json!({"config":{"value":[{"key":"valid","value":{"value":32}},
        {"key":"negative","value":{"value":-1}},{"key":"huge","value":{"value":100001}},
        {"key":"wrong_type","value":{"value":"32"}}]}}),
    ));
    assert_eq!(app_limits(&api).await, json!({"valid":32}));
}

#[tokio::test(start_paused = true)]
async fn app_config_lookup_has_a_ten_second_budget() {
    let mut api = Api::returning(None);
    api.pending = true;
    let start = tokio::time::Instant::now();
    assert_eq!(app_limits(&api).await, json!({}));
    assert_eq!(start.elapsed(), std::time::Duration::from_secs(10));
}
