use super::*;
use std::{
    collections::HashSet,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::time::{Instant, timeout};
static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
struct Slot(String);
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() {
            active.remove(&self.0);
        }
    }
}
struct Clear {
    slot: Option<Slot>,
    api: Option<Arc<dyn Telegram>>,
    request: Value,
    guard: Option<Arc<dyn Send + Sync>>,
}
impl Drop for Clear {
    fn drop(&mut self) {
        if let (Some(slot), Some(api), Ok(runtime)) = (
            self.slot.take(),
            self.api.take(),
            tokio::runtime::Handle::try_current(),
        ) {
            let req = self.request.take();
            let guard = self.guard.take();
            runtime.spawn(async move {
                let _slot = slot;
                let _guard = guard;
                let _ = timeout(
                    Duration::from_secs(3),
                    api.invoke("messages.setTyping", req),
                )
                .await;
            });
        }
    }
}
pub(super) async fn run(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let (seconds, topic) = validate_typing(a)?;
    let peer = args::peer(api, a, "chat_id").await?;
    let key = json!([
        api.cache_key(),
        a["account"],
        crate::server::access::marked_peer(&peer).unwrap_or_else(|| peer.to_string()),
        topic
    ])
    .to_string();
    ensure!(
        ACTIVE
            .lock()
            .map_err(|_| anyhow::anyhow!("Typing state unavailable"))?
            .insert(key.clone()),
        "Another typing indicator is active in this chat or topic"
    );
    let mut req = json!({"peer":peer,"action":{"_":"sendMessageTypingAction"}});
    if let Some(topic) = topic {
        req["top_msg_id"] = json!(topic);
    }
    let mut stop = req.clone();
    stop["action"] = json!({"_":"sendMessageCancelAction"});
    let mut clear = Clear {
        slot: Some(Slot(key)),
        api: api.clone_handle(),
        request: stop.clone(),
        guard: api.operation_guard(),
    };
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    loop {
        api.invoke("messages.setTyping", req.clone()).await?;
        let now = Instant::now();
        if now >= end {
            break;
        }
        tokio::time::sleep_until((now + Duration::from_secs(4)).min(end)).await;
        if Instant::now() >= end {
            break;
        }
    }
    let result = timeout(
        Duration::from_secs(3),
        api.invoke("messages.setTyping", stop),
    )
    .await;
    clear.api = None;
    result.map_err(|_| anyhow::anyhow!("Timed out clearing typing indicator"))?
}
