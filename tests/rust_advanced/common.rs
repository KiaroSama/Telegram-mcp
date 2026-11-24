#![allow(dead_code, unused_imports)]
//! Behavioral ports use mocked Telegram calls; no live account or credentials.
pub use anyhow::{Result, bail};
pub use async_trait::async_trait;
pub use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};
pub use telegram_mcp::{backend::Telegram, tools::advanced};

pub struct Mock {
    pub peer: Value,
    cache: Option<String>,
    responses: Mutex<HashMap<String, VecDeque<Value>>>,
    calls: Mutex<Vec<(String, Value)>>,
}
impl Mock {
    pub fn new() -> Self {
        Self {
            peer: json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"99"}),
            cache: None,
            responses: Mutex::new(HashMap::new()),
            calls: Mutex::new(vec![]),
        }
    }
    pub fn cached(mut self, key: &str) -> Self {
        self.cache = Some(key.into());
        self
    }
    pub fn respond(self, method: &str, result: Value) -> Self {
        self.responses
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(result);
        self
    }
    pub fn forum(self) -> Self {
        self.respond("channels.getChannels",json!({"chats":[{"_":"channel","id":"7","access_hash":"99","megagroup":true,"forum":true}]}))
    }
    pub fn request(&self, name: &str) -> Value {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null)
    }
    pub fn count(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .count()
    }
    pub fn all(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}
#[async_trait]
impl Telegram for Mock {
    fn cache_key(&self) -> Option<String> {
        self.cache.clone()
    }
    async fn upload(&self, _path: &std::path::Path) -> Result<Value> {
        Ok(json!({"_":"inputFile","id":"42","parts":1,"name":"photo.jpg","md5_checksum":""}))
    }
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(method, &params)?;
        self.calls.lock().unwrap().push((method.into(), params));
        let mut responses = self.responses.lock().unwrap();
        if let Some(queue) = responses.get_mut(method)
            && let Some(v) = queue.pop_front()
        {
            if v.get("$error").is_some() {
                bail!("Synthetic Telegram refusal");
            }
            return Ok(v);
        }
        Ok(json!({}))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        Ok(self.peer.clone())
    }
}
pub async fn run(api: &Mock, name: &str, args: Value) -> Result<Value> {
    advanced::call(api, name, &args).await.expect("owned tool")
}
pub fn poll() -> Value {
    json!({"_":"poll","id":"12","hash":"0","question":{"_":"textWithEntities","text":"Choose","entities":[]},"answers":[{"_":"pollAnswer","text":{"_":"textWithEntities","text":"A","entities":[]},"option":[19,85]},{"_":"pollAnswer","text":{"_":"textWithEntities","text":"B","entities":[]},"option":[241,2]}],"public_voters":true})
}
pub fn message(p: Value) -> Value {
    json!({"messages":[{"_":"message","id":20,"media":{"_":"messageMediaPoll","poll":p,"results":{"_":"pollResults","total_voters":4,"results":[{"_":"pollAnswerVoters","option":[241,2],"voters":3,"chosen":true},{"_":"pollAnswerVoters","option":[19,85],"voters":1}]}}}]})
}
pub fn with_poll(p: Value) -> Mock {
    Mock::new().respond("channels.getMessages", message(p))
}
pub fn folder(include: Value, pinned: Value) -> Value {
    json!({"filters":[{"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"Work","entities":[]},"include_peers":include,"pinned_peers":pinned,"exclude_peers":[],"groups":true,"exclude_archived":true,"color":3}]})
}
pub fn quiz_args() -> Value {
    json!({"chat_id":7,"question":"2+2?","options":["3","4"],"quiz_mode":true,"correct_option_index":1})
}
