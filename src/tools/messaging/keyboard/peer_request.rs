use super::*;
use anyhow::Context;
pub(super) async fn validate(
    api: &dyn Telegram,
    p: &Value,
    rule: &Value,
    bot: &Value,
) -> Result<()> {
    if rule.is_null() {
        return Ok(());
    }
    let kind = rule["_"].as_str().context("Missing requested peer type")?;
    let check = |field: &str, actual: bool| -> Result<()> {
        if let Some(wanted) = rule[field].as_bool() {
            ensure!(
                wanted == actual,
                "Requested peer does not meet {field} constraint"
            );
        }
        Ok(())
    };
    if kind == "requestPeerTypeUser" {
        ensure!(
            matches!(p["_"].as_str(), Some("inputPeerUser" | "inputPeerSelf")),
            "Requested peer must be a user"
        );
        if ["bot", "premium"].iter().any(|k| rule[k].is_boolean()) {
            let users = api
                .invoke("users.getUsers", json!({"id":[input_user(p)?]}))
                .await?;
            let u = users
                .as_array()
                .and_then(|us| us.first())
                .filter(|u| u["_"] == "user")
                .context("Requested user could not be verified")?;
            check("bot", u["bot"] == true)?;
            check("premium", u["premium"] == true)?;
        }
        return Ok(());
    }
    ensure!(
        matches!(kind, "requestPeerTypeChat" | "requestPeerTypeBroadcast"),
        "Unsupported requested peer type"
    );
    let entity = if let Some(c) = channel(p) {
        let r = api
            .invoke("channels.getChannels", json!({"id":[c]}))
            .await?;
        r["chats"]
            .as_array()
            .and_then(|xs| xs.iter().find(|e| e["id"] == p["channel_id"]))
            .cloned()
    } else {
        ensure!(
            p["_"] == "inputPeerChat" && kind == "requestPeerTypeChat",
            "Requested peer must be a group or channel"
        );
        let r = api
            .invoke("messages.getChats", json!({"id":[p["chat_id"]]}))
            .await?;
        r["chats"]
            .as_array()
            .and_then(|xs| xs.iter().find(|e| e["id"] == p["chat_id"]))
            .cloned()
    }
    .context("Requested group or channel could not be verified")?;
    ensure!(
        matches!(entity["_"].as_str(), Some("chat" | "channel"))
            && (kind == "requestPeerTypeBroadcast") == (entity["broadcast"] == true),
        "Requested chat type does not match"
    );
    check(
        "has_username",
        entity["username"].as_str().is_some_and(|s| !s.is_empty()),
    )?;
    check("forum", entity["forum"] == true)?;
    if rule["creator"] == true {
        ensure!(
            entity["creator"] == true,
            "Requested chat creator rights are missing"
        );
    }
    if let Some(rights) = rule["user_admin_rights"].as_object() {
        for (key, v) in rights {
            ensure!(
                v != true || entity["creator"] == true || entity["admin_rights"][key] == true,
                "Requested administrator rights are missing"
            );
        }
    }
    if rule["bot_participant"] == true || !rule["bot_admin_rights"].is_null() {
        ensure!(
            bot["_"] == "inputPeerUser",
            "Requesting bot identity is unavailable"
        );
        let bid = args::integer(bot, "user_id", -1)?;
        ensure!(bid > 0, "Invalid requesting bot");
        let participant = if let Some(channel) = channel(p) {
            let r = api
                .invoke(
                    "channels.getParticipant",
                    json!({"channel":channel,"participant":bot}),
                )
                .await?;
            r["participant"].clone()
        } else {
            let r = api
                .invoke("messages.getFullChat", json!({"chat_id":p["chat_id"]}))
                .await?;
            r["full_chat"]["participants"]["participants"]
                .as_array()
                .and_then(|xs| {
                    xs.iter()
                        .find(|v| args::integer(v, "user_id", -1).ok() == Some(bid))
                })
                .cloned()
                .context("Bot is not a verified group member")?
        };
        ensure!(
            args::integer(&participant, "user_id", -1)? == bid
                && matches!(
                    participant["_"].as_str(),
                    Some(
                        "channelParticipant"
                            | "channelParticipantAdmin"
                            | "channelParticipantCreator"
                            | "chatParticipant"
                            | "chatParticipantAdmin"
                            | "chatParticipantCreator"
                    )
                ),
            "Bot membership could not be verified"
        );
        if let Some(required) = rule["bot_admin_rights"].as_object() {
            for (k, v) in required {
                ensure!(
                    v != true
                        || matches!(
                            participant["_"].as_str(),
                            Some("channelParticipantCreator" | "chatParticipantCreator")
                        )
                        || participant["admin_rights"][k] == true,
                    "Bot lacks the requested administrator rights"
                );
            }
        }
    }
    Ok(())
}
