//! Behavioral ports of contact/privacy/admin/invite regression cases.
//! Every recorded request also crosses the pinned real TL encoder.
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Mutex,
};
use telegram_mcp::{backend::Telegram, tl::Schema, tools::identity};

type QueuedReplies = HashMap<String, VecDeque<std::result::Result<Value, String>>>;

#[derive(Default)]
pub(crate) struct Mock {
    calls: Mutex<Vec<(String, Value)>>,
    replies: Mutex<QueuedReplies>,
    peers: Mutex<HashMap<String, Value>>,
    resolved: Mutex<Vec<Value>>,
}
impl Mock {
    pub(crate) fn new() -> Self {
        let m = Self::default();
        m.set_peer(
            "7",
            json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}),
        );
        m.set_peer(
            "8",
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
        );
        m.set_peer("9", json!({"_":"inputPeerChat","chat_id":"9"}));
        m.set_peer("me", json!({"_":"inputPeerSelf"}));
        m
    }
    pub(crate) fn set_peer(&self, key: &str, p: Value) {
        self.peers.lock().unwrap().insert(key.into(), p);
    }
    pub(crate) fn reply(&self, method: &str, value: Value) {
        self.replies
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(Ok(value));
    }
    pub(crate) fn fail(&self, method: &str) {
        self.replies
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(Err("Injected RPC refusal".into()));
    }
    pub(crate) fn sent(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
    pub(crate) fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }
    pub(crate) fn resolutions(&self) -> Vec<Value> {
        self.resolved.lock().unwrap().clone()
    }
    pub(crate) fn info(&self, info: Value) {
        self.reply(
            "channels.getFullChannel",
            json!({"full_chat":{"_":"channelFull","id":"7"},"chats":[info]}),
        );
    }
}
#[async_trait]
impl Telegram for Mock {
    fn alias_matches(&self, _query: &str) -> Result<Value> {
        self.replies
            .lock()
            .unwrap()
            .get_mut("local.alias_matches")
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| Ok(json!([])))
            .map_err(anyhow::Error::msg)
    }
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        Schema::global()
            .encode_call(method, &params)
            .map_err(|e| anyhow::anyhow!("Invalid {method} request: {e:#}"))?;
        self.calls.lock().unwrap().push((method.into(), params));
        if let Some(r) = self
            .replies
            .lock()
            .unwrap()
            .get_mut(method)
            .and_then(VecDeque::pop_front)
        {
            return r.map_err(anyhow::Error::msg);
        }
        Ok(match method {
            "channels.getFullChannel" => {
                json!({"chats":[{"_":"channel","id":"7","access_hash":"70","title":"Group","megagroup":true,"forum":true}],"full_chat":{}})
            }
            "messages.getFullChat" => {
                json!({"chats":[{"_":"chat","id":"9","title":"Basic"}],"full_chat":{}})
            }
            "channels.getParticipant" => {
                json!({"participant":{"_":"channelParticipantAdmin","user_id":"8","admin_rights":{"_":"chatAdminRights","delete_messages":true,"ban_users":true,"other":true},"rank":"Keeper"}})
            }
            "messages.getPeerDialogs" => {
                json!({"dialogs":[{"_":"dialog","peer":{"_":"peerChannel","channel_id":"7"},"unread_count":3}],"users":[],"chats":[]})
            }
            "users.getUsers" => json!([{"_":"user","id":"8","bot":true}]),
            "contacts.getContacts" => json!({"users":[]}),
            "communities.getJoinedCommunities" => {
                json!({"chats":[{"_":"community","id":"7","access_hash":"70","title":"Community"}]})
            }
            _ => Value::Bool(true),
        })
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.resolved.lock().unwrap().push(input.clone());
        if input.is_object() {
            return Ok(input.clone());
        }
        let key = input
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| input.to_string());
        self.peers
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown peer"))
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        self.calls.lock().unwrap().push((
            "local.upload".into(),
            json!({"path":path.to_string_lossy()}),
        ));
        Ok(json!({"_":"inputFile","id":"1","parts":1,"name":"photo.png","md5_checksum":""}))
    }
    async fn download(
        &self,
        _location: Value,
        _destination: &Path,
        _max_bytes: u64,
    ) -> Result<Value> {
        bail!("No download expected")
    }
}
pub(crate) async fn run(m: &Mock, name: &str, args: Value) -> Result<Value> {
    identity::call(m, name, &args)
        .await
        .expect("Tool must be handled")
}
