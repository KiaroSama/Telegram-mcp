use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::Mutex,
};
use telegram_mcp::backend::Telegram;
pub struct Api {
    pub responses: Mutex<VecDeque<(&'static str, Result<Value>)>>,
    pub calls: Mutex<Vec<(String, Value)>>,
    pub resolved: Mutex<Vec<Value>>,
    pub uploaded: Mutex<Vec<std::path::PathBuf>>,
    pub identities: BTreeMap<String, Value>,
    pub session: &'static str,
}
impl Api {
    pub fn new(steps: Vec<(&'static str, Value)>) -> Self {
        Self {
            responses: Mutex::new(
                steps
                    .into_iter()
                    .map(|(name, value)| (name, Ok(value)))
                    .collect(),
            ),
            calls: Mutex::new(vec![]),
            resolved: Mutex::new(vec![]),
            uploaded: Mutex::new(vec![]),
            identities: BTreeMap::new(),
            session: "fixture-session",
        }
    }
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
    pub fn identity(mut self, key: Value, peer: Value) -> Self {
        self.identities.insert(key.to_string(), peer);
        self
    }
    pub fn error(&self, name: &'static str, error: &str) {
        self.responses
            .lock()
            .unwrap()
            .push_back((name, Err(anyhow::anyhow!(error.to_owned()))));
    }
    pub fn push(&self, name: &'static str, response: Value) {
        self.responses
            .lock()
            .unwrap()
            .push_back((name, Ok(response)));
    }
}
#[async_trait]
impl Telegram for Api {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.to_owned(), args));
        let (expected, value) = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected Telegram request");
        assert_eq!(name, expected, "Unexpected Telegram request");
        value
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.resolved.lock().unwrap().push(input.clone());
        Ok(self
            .identities
            .get(&input.to_string())
            .cloned()
            .unwrap_or_else(|| json!({"_":"inputPeerUser","user_id":"7","access_hash":"13"})))
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        ensure!(path.is_file(), "Fixture upload source must exist");
        self.uploaded.lock().unwrap().push(path.to_owned());
        Ok(
            json!({"_":"inputFile","id":"42","parts":1,"name":path.file_name().unwrap().to_string_lossy(),"md5_checksum":format!("{:x}",md5::compute(std::fs::read(path)?))}),
        )
    }
    fn cache_key(&self) -> Option<String> {
        Some(self.session.into())
    }
}
pub async fn run(api: &Api, name: &str, args: Value) -> Result<Value> {
    telegram_mcp::tools::messaging::call(api, name, &args)
        .await
        .expect("Missing native tool handler")
}
pub fn message(id: i64, text: &str) -> Value {
    json!({"_":"message","id":id,"message":text,"peer_id":{"_":"peerUser","user_id":"7"},"date":123,"entities":[]})
}
pub fn messages(items: Vec<Value>) -> Value {
    json!({"_":"messages.messages","messages":items,"users":[],"chats":[]})
}
pub fn keyboard(kind: Value) -> Value {
    let mut m = message(88, "");
    m["reply_markup"] = json!({"_":"replyKeyboardMarkup","rows":[{"_":"keyboardButtonRow","buttons":[{"_":"keyboardButton","text":"choose","type":kind}]}]});
    m
}
pub fn keyboard_args() -> Value {
    json!({"chat_id":7,"button_index":0,"expect_text":"choose","wait_seconds":0})
}
pub fn updates() -> Value {
    json!({"_":"updates","updates":[],"users":[],"chats":[],"date":123,"seq":1})
}
