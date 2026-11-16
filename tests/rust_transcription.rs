//! Native transcription protocol, cache, privacy and provider-boundary tests.
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use telegram_mcp::{
    backend::Telegram,
    tools::messaging::{
        self,
        transcribe::{self, Settings},
    },
};

struct Api {
    key: String,
    document: Mutex<Value>,
    replies: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<(String, Value)>>,
    downloads: AtomicUsize,
    text: String,
}
impl Api {
    fn new(text: &str) -> Self {
        static IDS: AtomicUsize = AtomicUsize::new(1);
        Self {
            key: format!(
                "transcription-fixture-{}",
                IDS.fetch_add(1, Ordering::Relaxed)
            ),
            document: Mutex::new(
                json!({"_":"document","id":"1234567890123456","access_hash":"77","file_reference":[1,2],"dc_id":2,"size":64,"mime_type":"audio/ogg","attributes":[{"_":"documentAttributeAudio","voice":true,"duration":12}]}),
            ),
            replies: Mutex::new(VecDeque::new()),
            calls: Mutex::new(vec![]),
            downloads: AtomicUsize::new(0),
            text: text.to_owned(),
        }
    }
    fn count(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == name)
            .count()
    }
}
#[async_trait]
impl Telegram for Api {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(method, &params)?;
        self.calls.lock().unwrap().push((method.into(), params));
        match method {
            "messages.getMessages" => Ok(
                json!({"_":"messages.messages","messages":[{"_":"message","id":10,"peer_id":{"_":"peerUser","user_id":"7"},"media":{"_":"messageMediaDocument","document":self.document.lock().unwrap().clone()},"entities":[]}]}),
            ),
            "messages.transcribeAudio" => {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                Ok(self.replies.lock().unwrap().pop_front().unwrap_or(json!({"_":"messages.transcribedAudio","pending":false,"text":self.text,"transcription_id":"9007199254740999"})))
            }
            "messages.rateTranscribedAudio" => Ok(json!(true)),
            "channels.getSendAs" => Ok(json!({"peers":[]})),
            _ => bail!("Unexpected test API call"),
        }
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerUser","user_id":"7","access_hash":"11"}))
    }
    async fn download_memory(&self, _: Value, max_bytes: u64) -> Result<Vec<u8>> {
        self.downloads.fetch_add(1, Ordering::Relaxed);
        let bytes = b"OggS synthetic voice fixture, not a real recording".to_vec();
        assert!(bytes.len() as u64 <= max_bytes);
        Ok(bytes)
    }
    async fn download(&self, _: Value, _: &Path, _: u64) -> Result<Value> {
        bail!("Transcription must not fall back to disk")
    }
    fn cache_key(&self) -> Option<String> {
        Some(self.key.clone())
    }
}
fn settings(values: &[(&str, &str)], args: &Value) -> Settings {
    Settings::from_values(
        &values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
        args,
    )
    .unwrap()
}
fn args() -> Value {
    json!({"chat_id":7,"message_id":10})
}
#[tokio::test]
async fn default_engine_is_telegram_and_no_audio_is_downloaded() {
    let api = Api::new("hello");
    let result = transcribe::transcribe_with(&api, &settings(&[], &args()), &args())
        .await
        .unwrap();
    assert_eq!(result["source"], "telegram");
    assert_eq!(result["text"], "hello");
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
    assert_eq!(api.count("messages.transcribeAudio"), 1);
    assert!(
        api.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(name, _)| !name.to_ascii_lowercase().contains("read"))
    );
}
#[tokio::test]
async fn successful_transcript_is_cached_without_another_engine_call() {
    let api = Api::new("hello");
    let config = settings(&[], &args());
    assert_eq!(
        transcribe::transcribe_with(&api, &config, &args())
            .await
            .unwrap()["cached"],
        false
    );
    assert_eq!(
        transcribe::transcribe_with(&api, &config, &args())
            .await
            .unwrap()["cached"],
        true
    );
    assert_eq!(api.count("messages.transcribeAudio"), 1);
    assert_eq!(api.count("messages.getMessages"), 2);
}
#[tokio::test]
async fn singleflight_prevents_duplicate_engine_requests() {
    let api = Arc::new(Api::new("one recording"));
    let config = Arc::new(settings(&[], &args()));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let api = Arc::clone(&api);
        let config = Arc::clone(&config);
        tasks.spawn(async move {
            transcribe::transcribe_with(api.as_ref(), &config, &args())
                .await
                .unwrap()
        });
    }
    let mut results = vec![];
    while let Some(result) = tasks.join_next().await {
        results.push(result.unwrap());
    }
    assert_eq!(api.count("messages.transcribeAudio"), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result["cached"] == false)
            .count(),
        1
    );
}
#[tokio::test]
async fn cache_is_bound_to_the_actual_account_session() {
    let first = Api::new("account one");
    let second = Api::new("account two");
    let config = settings(&[], &args());
    assert_eq!(
        transcribe::transcribe_with(&first, &config, &args())
            .await
            .unwrap()["text"],
        "account one"
    );
    let result = transcribe::transcribe_with(&second, &config, &args())
        .await
        .unwrap();
    assert_eq!(result["text"], "account two");
    assert_eq!(result["cached"], false);
}
#[tokio::test]
async fn replacing_message_media_invalidates_the_cache() {
    let api = Api::new("text");
    let config = settings(&[], &args());
    transcribe::transcribe_with(&api, &config, &args())
        .await
        .unwrap();
    api.document.lock().unwrap()["id"] = json!("1234567890123457");
    assert_eq!(
        transcribe::transcribe_with(&api, &config, &args())
            .await
            .unwrap()["cached"],
        false
    );
    assert_eq!(api.count("messages.transcribeAudio"), 2);
}
#[tokio::test]
async fn ordinary_audio_is_not_treated_as_a_voice_note() {
    let api = Api::new("text");
    api.document.lock().unwrap()["attributes"][0]["voice"] = json!(false);
    assert!(
        transcribe::transcribe_with(&api, &settings(&[], &args()), &args())
            .await
            .is_err()
    );
    assert_eq!(api.count("messages.transcribeAudio"), 0);
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn video_notes_are_transcribable_without_marking_them_seen() {
    let api = Api::new("video");
    api.document.lock().unwrap()["attributes"] =
        json!([{"_":"documentAttributeVideo","round_message":true,"duration":3.5,"w":240,"h":240}]);
    let result = transcribe::transcribe_with(&api, &settings(&[], &args()), &args())
        .await
        .unwrap();
    assert_eq!(result["text"], "video");
    assert_eq!(result["duration"], 3.5);
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn disabled_transcription_never_contacts_telegram_or_an_engine() {
    let api = Api::new("text");
    let result = transcribe::transcribe_with(
        &api,
        &settings(&[("TELEGRAM_TRANSCRIBE", "off")], &args()),
        &args(),
    )
    .await
    .unwrap();
    assert_eq!(result["reason"], "transcription_disabled");
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test(start_paused = true)]
async fn pending_telegram_result_is_polled_until_ready() {
    let api = Api::new("ready");
    api.replies
        .lock()
        .unwrap()
        .push_back(json!({"pending":true,"transcription_id":"9007199254740999"}));
    let result = transcribe::transcribe_with(&api, &settings(&[], &args()), &args())
        .await
        .unwrap();
    assert_eq!(result["text"], "ready");
    assert_eq!(api.count("messages.transcribeAudio"), 2);
}
#[tokio::test(start_paused = true)]
async fn persistent_pending_result_is_not_cached_as_final_text() {
    let api = Api::new("ready");
    for _ in 0..11 {
        api.replies
            .lock()
            .unwrap()
            .push_back(json!({"pending":true,"transcription_id":"9007199254740999"}));
    }
    let config = settings(&[], &args());
    assert_eq!(
        transcribe::transcribe_with(&api, &config, &args())
            .await
            .unwrap()["reason"],
        "pending"
    );
    assert_eq!(
        transcribe::transcribe_with(&api, &config, &args())
            .await
            .unwrap()["text"],
        "ready"
    );
    assert_eq!(api.count("messages.transcribeAudio"), 12);
}
#[tokio::test]
async fn transcript_output_explicitly_marks_machine_text() {
    let api = Api::new("ماشین 東京");
    let result = transcribe::transcribe_with(&api, &settings(&[], &args()), &args())
        .await
        .unwrap();
    assert_eq!(result["text"], "ماشین 東京");
    assert!(
        result["note"]
            .as_str()
            .unwrap()
            .contains("not a verbatim quote")
    );
}
#[tokio::test]
async fn unconfigured_hosted_engine_refuses_before_downloading() {
    let api = Api::new("text");
    let request = json!({"chat_id":7,"message_id":10,"engine":"groq"});
    assert!(
        transcribe::transcribe_with(&api, &settings(&[], &request), &request)
            .await
            .unwrap_err()
            .to_string()
            .contains("GROQ_API_KEY")
    );
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn hosted_declared_size_is_checked_before_any_download() {
    let api = Api::new("text");
    api.document.lock().unwrap()["size"] = json!(25 * 1024 * 1024 + 1);
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let config = settings(
        &[("TELEGRAM_TRANSCRIBE_OPENAI_URL", "http://127.0.0.1:9/v1")],
        &request,
    );
    assert!(
        transcribe::transcribe_with(&api, &config, &request)
            .await
            .unwrap_err()
            .to_string()
            .contains("25MiB")
    );
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn missing_native_whisper_configuration_never_downloads_a_model() {
    let api = Api::new("text");
    let request = json!({"chat_id":7,"message_id":10,"engine":"whisper"});
    let result = transcribe::transcribe_with(&api, &settings(&[], &request), &request)
        .await
        .unwrap_err();
    assert!(result.to_string().contains("whisper-cli"));
    assert_eq!(api.downloads.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn cached_text_cannot_bypass_topic_binding() {
    let api = Api::new("text");
    let config = settings(&[], &args());
    transcribe::transcribe_with(&api, &config, &args())
        .await
        .unwrap();
    let request = json!({"chat_id":7,"message_id":10,"topic_id":42});
    assert!(
        transcribe::transcribe_with(&api, &config, &request)
            .await
            .is_err()
    );
    assert_eq!(api.count("messages.transcribeAudio"), 1);
}
#[tokio::test]
async fn rating_uses_the_exact_64bit_transcription_id() {
    let api = Api::new("text");
    let request =
        json!({"chat_id":7,"message_id":10,"transcription_id":"9007199254740999","good":true});
    assert_eq!(
        transcribe::call(&api, "rate_voice_transcription", &request)
            .await
            .unwrap()
            .unwrap(),
        true
    );
    assert_eq!(
        api.calls.lock().unwrap()[0].1["transcription_id"],
        "9007199254740999"
    );
}
#[tokio::test]
async fn get_send_as_is_a_read_only_alias() {
    let api = Api::new("text");
    messaging::call(&api, "get_send_as", &json!({"chat_id":7}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(api.calls.lock().unwrap()[0].0, "channels.getSendAs");
}
#[test]
fn unknown_engine_is_refused() {
    let result = Settings::from_values(&BTreeMap::new(), &json!({"engine":"unknown"}));
    assert!(result.is_err());
}
#[test]
fn endpoint_embedded_credentials_are_refused() {
    let result = Settings::from_values(
        &[(
            "TELEGRAM_TRANSCRIBE_OPENAI_URL".into(),
            "https://user:private@example.com/v1".into(),
        )]
        .into_iter()
        .collect(),
        &json!({"engine":"openai"}),
    );
    assert!(result.is_err());
}

#[path = "rust_transcription/hosted.rs"]
mod hosted_tests;

#[test]
fn hosted_endpoint_security_rejects_cleartext_remote_audio_and_cleartext_keys() {
    for (endpoint, key) in [
        ("http://api.example.com/v1", None),
        ("http://localhost/v1", None),
        ("http://127.0.0.1/v1", Some("synthetic-key")),
        ("http://[::1]/v1", Some("synthetic-key")),
    ] {
        let mut values = BTreeMap::from([(
            "TELEGRAM_TRANSCRIBE_OPENAI_URL".to_owned(),
            endpoint.to_owned(),
        )]);
        if let Some(key) = key {
            values.insert(
                "TELEGRAM_TRANSCRIBE_OPENAI_API_KEY".to_owned(),
                key.to_owned(),
            );
        }
        let error = Settings::from_values(&values, &json!({"engine":"openai"}))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("HTTPS"));
        assert!(!error.contains("synthetic-key"));
    }
    for endpoint in [
        "https://api.example.com/v1",
        "http://127.0.0.1/v1",
        "http://[::1]/v1",
    ] {
        assert!(
            Settings::from_values(
                &BTreeMap::from([(
                    "TELEGRAM_TRANSCRIBE_OPENAI_URL".to_owned(),
                    endpoint.to_owned()
                )]),
                &json!({"engine":"openai"})
            )
            .is_ok()
        );
    }
}
