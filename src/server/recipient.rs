//! Per-operation private peer references shared by approval, policy and dispatch.
use super::*;
use std::path::Path;

#[derive(Clone)]
struct BoundApi {
    inner: Arc<dyn Telegram>,
    peers: Vec<(Value, String, Value)>,
}

pub(super) async fn bind(
    inner: Arc<dyn Telegram>,
    args: &Value,
) -> Result<(Arc<dyn Telegram>, Value)> {
    let mut bound = BoundApi {
        inner,
        peers: vec![],
    };
    let mut approval = args.clone();
    let mut originals = vec![];
    for field in ["chat_id", "to_chat_id", "from_chat_id"] {
        let Some(input) = args.get(field).filter(|v| !v.is_null()) else {
            continue;
        };
        let peer = bound.resolve_peer(input).await?;
        let id = if peer["_"] == "inputPeerSelf" {
            let own = bound
                .inner
                .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                .await?;
            let rows = own
                .as_array()
                .context("Could not establish the authenticated recipient identity")?;
            let row = rows
                .first()
                .filter(|_| rows.len() == 1)
                .filter(|row| row["_"] == "user")
                .context("Could not establish the authenticated recipient identity")?;
            row["id"]
                .as_str()
                .and_then(|id| id.parse::<i64>().ok())
                .or_else(|| row["id"].as_i64())
                .filter(|id| *id > 0)
                .context("The authenticated recipient identity is invalid")?
                .to_string()
        } else {
            access::marked_peer(&peer).context("The recipient has no stable Telegram identity")?
        };
        bound.peers.push((input.clone(), id.clone(), peer));
        approval[field] = json!(id);
        originals.push(input.clone());
    }
    if !originals.is_empty() {
        // Preserve checks for literal approval-channel names and tainted input.
        // Clients cannot inject this field: their original schema was validated.
        approval["_requested_peer_targets"] = json!(originals);
    }
    Ok((Arc::new(bound), approval))
}

#[async_trait::async_trait]
impl Telegram for BoundApi {
    fn pinned_identity(&self, peer: &Value) -> Option<String> {
        self.peers
            .iter()
            .find(|(_, _, pinned)| pinned == peer)
            .map(|(_, id, _)| id.clone())
            .or_else(|| self.inner.pinned_identity(peer))
    }
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        self.inner.invoke(method, params).await
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        if let Some((_, _, peer)) = self.peers.iter().find(|(original, id, peer)| {
            original == input
                || peer == input
                || input.as_str() == Some(id.as_str())
                || input.as_i64().is_some_and(|n| n.to_string() == *id)
        }) {
            return Ok(peer.clone());
        }
        self.inner.resolve_peer(input).await
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        self.inner.upload(path).await
    }
    async fn download(&self, location: Value, path: &Path, max_bytes: u64) -> Result<Value> {
        self.inner.download(location, path, max_bytes).await
    }
    async fn download_memory(&self, location: Value, max_bytes: u64) -> Result<Vec<u8>> {
        self.inner.download_memory(location, max_bytes).await
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        self.inner.clone_handle().map(|inner| {
            Arc::new(Self {
                inner,
                peers: self.peers.clone(),
            }) as Arc<dyn Telegram>
        })
    }
    fn cache_key(&self) -> Option<String> {
        self.inner.cache_key()
    }
    fn alias_matches(&self, query: &str) -> Result<Value> {
        self.inner.alias_matches(query)
    }
}
