//! Account listing and session-health handlers enriched with live `get_me()`
//! identity for already-connected accounts without opening auxiliary logins.
use crate::backend::Telegram;
use anyhow::Result;
use serde_json::{Value, json};

pub async fn list_accounts(api: &dyn Telegram) -> Result<Value> {
    api.invoke("local.accounts", json!({})).await
}

pub async fn check_account_session(api: &dyn Telegram, args: &Value) -> Result<Value> {
    api.invoke(
        "local.session_health",
        json!({
            "config_key": crate::args::text(args, "config_key")?,
            "config_digest": crate::args::text(args, "config_digest")?,
        }),
    )
    .await
}
