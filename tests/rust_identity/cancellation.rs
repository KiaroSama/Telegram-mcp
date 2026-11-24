use super::common::Mock;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use telegram_mcp::{backend::Telegram, tools::identity};
use tokio::sync::Notify;

#[derive(Clone)]
struct OwnedMock {
    inner: Arc<Mock>,
    ejected: Arc<Notify>,
    release: Arc<Notify>,
    cleared: Arc<Notify>,
}
#[async_trait]
impl Telegram for OwnedMock {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        let eject =
            method == "channels.editBanned" && params["banned_rights"]["view_messages"] == true;
        let clear = method == "channels.editBanned" && !eject;
        let result = self.inner.invoke(method, params).await;
        if eject {
            self.ejected.notify_one();
            self.release.notified().await;
        }
        if clear {
            self.cleared.notify_one();
        }
        result
    }
    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        self.inner.resolve_peer(value).await
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
}

#[tokio::test]
async fn removal_finishes_ban_cleanup_after_the_mcp_caller_is_cancelled() {
    let mock = OwnedMock {
        inner: Arc::new(Mock::new()),
        ejected: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        cleared: Arc::new(Notify::new()),
    };
    let caller = {
        let api = mock.clone();
        tokio::spawn(async move {
            identity::call(&api, "remove_user", &json!({"chat_id":7,"user_id":8})).await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), mock.ejected.notified())
        .await
        .expect("Removal did not reach the synthetic ejection");
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    mock.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), mock.cleared.notified())
        .await
        .expect("Caller cancellation prevented ban cleanup");
    let sent = mock.inner.sent("channels.editBanned");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["banned_rights"]["view_messages"], true);
    assert!(sent[1]["banned_rights"].get("view_messages").is_none());
}
