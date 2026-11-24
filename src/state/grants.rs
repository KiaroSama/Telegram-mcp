use super::{State, normalize_key};
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl State {
    pub fn always_approvals(&self) -> Value {
        match self.read_json("always-approvals.json", json!({"grants":[],"folders":[]})) {
            Ok(value)
                if value["grants"].as_array().is_some_and(|rows| {
                    rows.iter().all(|row| {
                        row.as_array().is_some_and(|parts| {
                            parts.len() == 3 && parts.iter().all(Value::is_string)
                        })
                    })
                }) =>
            {
                value
            }
            _ => {
                json!({"grants":[],"folders":[],"error":"Always-approval store is unreadable; no grants are active"})
            }
        }
    }

    pub fn approval_is_granted(&self, account: &str, tool: &str, chat: &Value) -> bool {
        if chat.is_null() {
            return false;
        }
        let expected = json!([account.to_lowercase(), tool, normalize_key(chat)]);
        self.always_approvals()["grants"]
            .as_array()
            .is_some_and(|rows| rows.contains(&expected))
    }

    pub(crate) fn grant_approval(&self, account: &str, tool: &str, chat: &Value) -> Result<()> {
        self.update_json(
            "always-approvals.json",
            json!({"grants":[],"folders":[]}),
            |store| {
                let rows = store
                    .get_mut("grants")
                    .and_then(Value::as_array_mut)
                    .context("Always-approval store is invalid; existing bytes were preserved")?;
                let triple = json!([account.to_lowercase(), tool, normalize_key(chat)]);
                if !rows.contains(&triple) {
                    rows.push(triple);
                }
                Ok(())
            },
        )
    }

    pub fn revoke_approval(&self, account: &str, tool: &str, chat: &Value) -> Result<bool> {
        self.update_json(
            "always-approvals.json",
            json!({"grants":[],"folders":[]}),
            |store| {
                let rows = store
                    .get_mut("grants")
                    .and_then(Value::as_array_mut)
                    .context("Always-approval store is invalid")?;
                let expected = json!([account.to_lowercase(), tool, normalize_key(chat)]);
                let before = rows.len();
                rows.retain(|row| row != &expected);
                Ok(before != rows.len())
            },
        )
    }

    pub fn revoke_folder(&self, folder: &str) -> Result<bool> {
        let key = super::paths::canonical_existing(std::path::Path::new(folder))?;
        self.update_json(
            "always-approvals.json",
            json!({"grants":[],"folders":[]}),
            |store| {
                let rows = store
                    .get_mut("folders")
                    .and_then(Value::as_array_mut)
                    .context("Always-allowed folder store is invalid")?;
                let before = rows.len();
                rows.retain(|row| {
                    row.as_str()
                        .is_none_or(|value| std::path::Path::new(value) != key)
                });
                Ok(before != rows.len())
            },
        )
    }
}
