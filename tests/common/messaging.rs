#![allow(dead_code)]
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use telegram_mcp::{
    backend::Telegram,
    tools::messaging::{self},
};

pub struct Wire {
    peer: Value,
    pub replies: Mutex<VecDeque<Result<Value>>>,
    calls: Mutex<Vec<(String, Value)>>,
    pub resolves: AtomicUsize,
}
impl Wire {
    pub fn new(replies: Vec<Value>) -> Self {
        Self {
            peer: json!({"_":"inputPeerUser","user_id":"7","access_hash":"13"}),
            replies: Mutex::new(replies.into_iter().map(Ok).collect()),
            calls: Mutex::new(vec![]),
            resolves: AtomicUsize::new(0),
        }
    }
    pub fn channel(replies: Vec<Value>) -> Self {
        let mut w = Self::new(replies);
        w.peer = json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"13"});
        w
    }
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}
#[async_trait]
impl Telegram for Wire {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.to_owned(), args));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(json!({"_":"updates","updates":[]})))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        Ok(self.peer.clone())
    }
}
pub async fn run(w: &Wire, name: &str, args: Value) -> Result<Value> {
    messaging::call(w, name, &args)
        .await
        .unwrap_or_else(|| Err(anyhow!("Tool route is missing")))
}
pub fn message(id: i64, text: &str) -> Value {
    json!({"_":"message","id":id,"message":text,"peer_id":{"_":"peerUser","user_id":"7"},"entities":[]})
}
pub fn messages(ms: Vec<Value>) -> Value {
    json!({"_":"messages.messages","messages":ms,"users":[],"chats":[]})
}
pub fn bold(offset: i64, length: i64) -> Value {
    json!({"type":"bold","offset":offset,"length":length})
}
