use crate::backend::Telegram;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub async fn list_chat_commands(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let limit = crate::args::limit(args, "limit", 100, 200)? as usize;
    let peer = api
        .resolve_peer(crate::args::required(args, "chat_id")?)
        .await?;
    let kind = peer["_"].as_str().context("Resolved peer has no kind")?;
    let (group, full, infos) = match kind {
        "inputPeerUser" | "inputPeerSelf" => {
            let user = if kind == "inputPeerSelf" {
                json!({"_":"inputUserSelf"})
            } else {
                json!({"_":"inputUser","user_id":peer["user_id"],"access_hash":peer["access_hash"]})
            };
            let full = api.invoke("users.getFullUser", json!({"id":user})).await?;
            let infos = full["full_user"]
                .get("bot_info")
                .filter(|v| !v.is_null())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            (false, full, infos)
        }
        "inputPeerChat" => {
            let full = api
                .invoke("messages.getFullChat", json!({"chat_id":peer["chat_id"]}))
                .await?;
            let infos = full["full_chat"]["bot_info"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            (true, full, infos)
        }
        "inputPeerChannel" => {
            let full=api.invoke("channels.getFullChannel",json!({"channel":{"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]}})).await?;
            let infos = full["full_chat"]["bot_info"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            (true, full, infos)
        }
        _ => bail!("This peer has no bot command preview"),
    };
    let prefix = args["prefix"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches('/')
        .to_lowercase();
    let users = full["users"].as_array().cloned().unwrap_or_default();
    let mut entries = Vec::new();
    let mut bots = Vec::new();
    for info in infos {
        let username = users
            .iter()
            .find(|user| user["id"] == info["user_id"])
            .and_then(|user| user["username"].as_str());
        let handle = username.map(|name| format!("@{name}"));
        let commands = info["commands"].as_array().cloned().unwrap_or_default();
        bots.push(json!({"bot_user_id":info["user_id"],"bot":handle,"menu_button":info["menu_button"],"command_count":commands.len()}));
        for command in commands {
            let text = command["command"].as_str().unwrap_or("");
            if text.is_empty() || !text.to_lowercase().starts_with(&prefix) {
                continue;
            }
            let send_as = match (group, username) {
                (true, Some(name)) => format!("/{text}@{name}"),
                _ => format!("/{text}"),
            };
            entries.push(json!({"command":text,"description":command["description"],"bot":handle,"bot_user_id":info["user_id"],"send_as_text":send_as}));
        }
    }
    let total = entries.len();
    let note = if bots.is_empty() {
        Some("No bot is present in this chat.")
    } else if entries.is_empty() {
        Some("Bots are present but publish no matching command.")
    } else {
        None
    };
    Ok(
        json!({"commands":entries.into_iter().take(limit).collect::<Vec<_>>(),"chat_scope":if group{"group"}else{"private"},"bot_count":bots.len(),"bots":bots,"matched":total,"total":total,"has_more":total>limit,"note":note,"untrusted_content":true}),
    )
}
