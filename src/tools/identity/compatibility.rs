//! Current upstream entry points whose native implementation is shared.
use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub const NAMES: &[&str] = &[
    "mark_topic_as_read",
    "upgrade_group_to_supergroup",
    "set_default_chat_permissions",
];

pub async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    match name {
        "mark_topic_as_read" => mark_topic(api, args).await,
        "upgrade_group_to_supergroup" => {
            let p = peer(api, args, "chat_id").await?;
            let info = chat_info(api, &p).await?;
            if is_channel(&p) {
                ensure!(
                    info["megagroup"] == true,
                    "A broadcast channel cannot be upgraded from a basic group"
                );
                let id = int_value(&entity_id(&p)?, "channel_id")?;
                return Ok(
                    json!({"upgraded":false,"already_supergroup":true,"chat_id":marked_channel(id)?,"channel_id":id}),
                );
            }
            ensure!(
                kind(&p) == "inputPeerChat",
                "Only a basic group can be upgraded"
            );
            if let Some(migrated) = info.get("migrated_to").filter(|value| !value.is_null()) {
                let id = int_value(req(migrated, "channel_id")?, "channel_id")?;
                return Ok(
                    json!({"upgraded":false,"already_supergroup":true,"chat_id":marked_channel(id)?,"channel_id":migrated["channel_id"]}),
                );
            }
            let result = crate::tools::advanced::call(api, "upgrade_to_supergroup", args)
                .await
                .context("Native upgrade route is unavailable")??;
            let destination = result["chats"].as_array().and_then(|chats| {
                chats
                    .iter()
                    .find(|chat| kind(chat) == "channel" && chat["megagroup"] == true)
            });
            if let Some(destination) = destination {
                let id = int_value(req(destination, "id")?, "channel_id")?;
                Ok(
                    json!({"upgraded":true,"chat_id":marked_channel(id)?,"channel_id":id,"result":result}),
                )
            } else {
                Ok(
                    json!({"upgraded":true,"result":result,"warning":"The group was upgraded but Telegram did not return its new supergroup ID; inspect recent chats before any retry"}),
                )
            }
        }
        "set_default_chat_permissions" => default_permissions(api, args).await,
        _ => unreachable!(),
    }
}

fn marked_channel(id: i64) -> Result<String> {
    ensure!(id > 0, "Telegram returned an invalid channel ID");
    Ok((-1_000_000_000_000i64)
        .checked_sub(id)
        .context("Telegram channel ID exceeds its range")?
        .to_string())
}

async fn mark_topic(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let topic = int_value(req(args, "topic_id")?, "topic_id")?;
    ensure!(
        (1..=i32::MAX as i64).contains(&topic),
        "topic_id must be a positive 32-bit message id"
    );
    let max_id = int(args, "max_id", 0)?;
    ensure!(
        (0..=i32::MAX as i64).contains(&max_id),
        "max_id must be a nonnegative 32-bit message id"
    );
    let p = peer(api, args, "chat_id").await?;
    let info = chat_info(api, &p).await?;
    ensure!(
        is_channel(&p) && info["megagroup"] == true && info["forum"] == true,
        "The chat must be a forum-enabled supergroup"
    );
    let response = api
        .invoke(
            "messages.getForumTopicsByID",
            json!({"peer":p,"topics":[topic]}),
        )
        .await?;
    let found = response["topics"]
        .as_array()
        .and_then(|topics| {
            topics
                .iter()
                .find(|item| item["id"].as_i64() == Some(topic))
        })
        .context("The requested topic was not found")?;
    let top = int_value(req(found, "top_message")?, "top_message")?;
    ensure!(
        (1..=i32::MAX as i64).contains(&top),
        "The topic has no valid top message"
    );
    let read_max_id = if max_id == 0 { top } else { max_id.min(top) };
    // Layer233 has no readForumTopic method. readDiscussion is the official
    // topic-scoped receipt method used by the current upstream implementation.
    let result = api
        .invoke(
            "messages.readDiscussion",
            json!({"peer":p,"msg_id":topic,"read_max_id":read_max_id}),
        )
        .await?;
    Ok(json!({"marked_read":true,"topic_id":topic,"max_id":read_max_id,"result":result}))
}

async fn default_permissions(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let until = int(args, "until_date", 0)?;
    ensure!(
        (0..=i32::MAX as i64).contains(&until),
        "until_date must be a nonnegative 32-bit Unix timestamp"
    );
    let mut rights = json!({"_":"chatBannedRights","until_date":until});
    for (field, default) in [
        ("send_messages", true),
        ("send_media", true),
        ("send_stickers", true),
        ("send_gifs", true),
        ("send_games", true),
        ("send_inline", true),
        ("embed_links", true),
        ("send_polls", true),
        ("change_info", false),
        ("invite_users", true),
        ("pin_messages", false),
    ] {
        rights[field] = json!(!boolean(args, field, default)?);
    }
    let p = peer(api, args, "chat_id").await?;
    ensure!(
        is_channel(&p) || kind(&p) == "inputPeerChat",
        "Default permissions require a group or channel"
    );
    // Upstream exposes separate sticker/GIF/game/inline bits and an expiry;
    // retain each bit instead of collapsing them into the grouped native UI.
    api.invoke(
        "messages.editChatDefaultBannedRights",
        json!({"peer":p,"banned_rights":rights}),
    )
    .await
}
