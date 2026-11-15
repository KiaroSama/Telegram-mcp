use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "send_message"
        | "edit_message"
        | "reply_to_message"
        | "send_scheduled_message"
        | "schedule_message"
        | "edit_scheduled_message" => Some(send(api, name, a).await),
        "delete_message" | "delete_messages_bulk" | "delete_chat_history" => {
            Some(delete(api, name, a).await)
        }
        "forward_message" | "forward_messages" | "copy_message" => {
            Some(forward(api, name, a).await)
        }
        "save_draft"
        | "clear_draft"
        | "get_drafts"
        | "get_scheduled_messages"
        | "list_scheduled_messages"
        | "delete_scheduled_message"
        | "cancel_scheduled_message" => Some(queue(api, name, a).await),
        _ => None,
    }
}
async fn send(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let edit = matches!(name, "edit_message" | "edit_scheduled_message");
    let key = if name == "edit_message" {
        "new_text"
    } else if name == "reply_to_message" {
        "text"
    } else {
        "message"
    };
    let message = if name == "edit_scheduled_message" {
        a.get(key)
            .filter(|v| !v.is_null())
            .map(|_| args::text(a, key))
            .transpose()?
            .unwrap_or("")
    } else {
        args::text(a, key)?
    };
    let keep_scheduled_body =
        name == "edit_scheduled_message" && a.get("message").is_none_or(Value::is_null);
    ensure!(
        name == "edit_scheduled_message" || !message.trim().is_empty(),
        "Message text cannot be empty"
    );
    let mid = if edit || name == "reply_to_message" {
        Some(id(a, "message_id")?)
    } else {
        None
    };
    let schedule_key = if name == "send_scheduled_message" {
        "schedule_date"
    } else {
        "when"
    };
    let when = schedule(a, schedule_key)?;
    ensure!(
        !matches!(name, "send_scheduled_message" | "schedule_message") || when.is_some(),
        "A scheduled send requires a future date"
    );
    let repeat = repeat(a)?;
    let mut request = json!({"message":message});
    let mode = a["parse_mode"].as_str().unwrap_or("").to_ascii_lowercase();
    if mode.starts_with("rich_") {
        ensure!(
            matches!(mode.as_str(), "rich_html" | "rich_markdown"),
            "parse_mode must be html, markdown, rich_html, rich_markdown, or omitted"
        );
        ensure!(
            a.get("entities").is_none_or(|v| v.is_null()),
            "Rich content cannot be combined with explicit entities"
        );
        let mut rich = if mode == "rich_html" {
            json!({"_":"inputRichMessageHTML","html":message})
        } else {
            json!({"_":"inputRichMessageMarkdown","markdown":message})
        };
        copy_optional(
            &mut rich,
            a,
            &[("rich_rtl", "rtl"), ("rich_no_autolink", "noautolink")],
        );
        if let Some(files) = a.get("rich_files").filter(|v| !v.is_null()) {
            ensure!(
                files.is_object(),
                "rich_files must map attachment ids to paths"
            );
            ensure!(
                files.as_object().unwrap().len() <= 20,
                "rich_files cannot contain more than 20 attachments"
            );
        }
        request["rich_message"] = rich;
    } else if let Some(raw) = a
        .get("entities")
        .filter(|v| !v.is_null() && !keep_scheduled_body)
    {
        ensure!(
            mode.is_empty() || mode == "none",
            "Explicit entities cannot be combined with parse_mode"
        );
        request["entities"] = json!(build_entities(api, message, raw).await?);
    } else if !mode.is_empty() && mode != "none" {
        let (text, entities) = crate::backend::parse_text(message, Some(&mode))?;
        request["message"] = json!(text);
        request["entities"] = json!(entities);
    }
    ensure!(
        request["message"]
            .as_str()
            .unwrap_or("")
            .encode_utf16()
            .count()
            <= 4096
            || mode.starts_with("rich_"),
        "Message text exceeds Telegram's 4096 UTF-16 unit limit"
    );
    let p = args::peer(api, a, "chat_id").await?;
    request["peer"] = p.clone();
    if name == "edit_scheduled_message" {
        preserve_scheduled(
            api,
            &p,
            mid.unwrap(),
            a,
            &mut request,
            when,
            keep_scheduled_body,
        )
        .await?;
    }
    if let Some(files) = a
        .get("rich_files")
        .and_then(Value::as_object)
        .filter(|files| !files.is_empty())
    {
        ensure!(
            mode.starts_with("rich_"),
            "rich_files requires rich_html or rich_markdown"
        );
        let state = crate::state::State::global()?;
        let mut references = vec![];
        for (name, path) in files {
            let path = state.resolve_file(
                std::path::Path::new(
                    path.as_str()
                        .ok_or_else(|| anyhow::anyhow!("Rich attachment path must be a string"))?,
                ),
                crate::state::FileAccess::Read,
            )?;
            let inferred = super::media::uploads::infer_kind(&path);
            let kind = if inferred == "photo" {
                "photo"
            } else {
                "document"
            };
            let media = super::media::uploads::uploaded(api, &path, kind, None).await?;
            let uploaded = api
                .invoke("messages.uploadMedia", json!({"peer":p,"media":media}))
                .await?;
            let reference = if let Some(photo) = uploaded.get("photo") {
                json!({"_":"inputRichFilePhoto","id":name,"photo":{"_":"inputPhoto","id":photo["id"],"access_hash":photo["access_hash"],"file_reference":photo["file_reference"]}})
            } else {
                let doc = uploaded.get("document").ok_or_else(|| {
                    anyhow::anyhow!("Telegram omitted the uploaded attachment reference")
                })?;
                json!({"_":"inputRichFileDocument","id":name,"document":{"_":"inputDocument","id":doc["id"],"access_hash":doc["access_hash"],"file_reference":doc["file_reference"]}})
            };
            references.push(reference);
        }
        request["rich_message"]["files"] = json!(references);
    }
    if edit {
        request["id"] = json!(mid.unwrap());
    } else {
        request["random_id"] = args::random_id();
        if let Some(mut reply) = reply(
            a,
            if name == "reply_to_message" {
                mid
            } else {
                None
            },
        )? {
            if let Some(quote) = a.get("quote_text").filter(|v| !v.is_null()) {
                let quote = quote
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("quote_text must be a string"))?;
                ensure!(!quote.is_empty(), "quote_text cannot be empty");
                let rich_requested = ["quote_block_index", "quote_row", "quote_column"]
                    .iter()
                    .any(|key| a.get(*key).is_some_and(|v| !v.is_null()));
                let rich_result = api
                    .invoke(
                        "messages.getRichMessage",
                        json!({"peer":p,"id":reply["reply_to_msg_id"]}),
                    )
                    .await;
                if let Err(error) = &rich_result {
                    ensure!(
                        error.to_string().contains("MESSAGE_ID_INVALID"),
                        "Could not inspect the source rich message safely: {error}"
                    );
                }
                if let Ok(result) = &rich_result
                    && let Some(rich) = result_messages(result)
                        .first()
                        .and_then(|message| message.get("rich_message"))
                        .filter(|rich| !rich.is_null())
                {
                    let (fragments, offset) = super::rich_quote::select(rich, a)?;
                    reply["quote_rich"] = json!(fragments);
                    reply["quote_offset"] = json!(offset);
                    request["reply_to"] = reply.clone(); // Rich fragments carry their own exact formatting.
                }
                if request["reply_to"].get("quote_rich").is_none() {
                    ensure!(
                        !rich_requested,
                        "Quote block/row/column coordinates require a rich source"
                    );
                    let original =
                        fetch(api, &p, reply["reply_to_msg_id"].as_i64().unwrap()).await?;
                    let source = original["message"].as_str().unwrap_or("");
                    let offset = if a.get("quote_offset").is_some_and(|v| !v.is_null()) {
                        args::integer(a, "quote_offset", -1)?
                    } else {
                        source
                            .find(quote)
                            .map(|index| source[..index].encode_utf16().count() as i64)
                            .ok_or_else(|| {
                                anyhow::anyhow!("quote_text is not present in the replied message")
                            })?
                    };
                    let units: Vec<u16> = source.encode_utf16().collect();
                    let quote_units: Vec<u16> = quote.encode_utf16().collect();
                    ensure!(
                        offset >= 0
                            && units.get(offset as usize..offset as usize + quote_units.len())
                                == Some(quote_units.as_slice()),
                        "quote_offset does not select quote_text in the replied message"
                    );
                    reply["quote_text"] = json!(quote);
                    reply["quote_offset"] = json!(offset);
                    let mut quoted_entities = vec![];
                    for entity in original["entities"].as_array().into_iter().flatten() {
                        let begin = entity["offset"].as_i64().unwrap_or(0);
                        let end = begin + entity["length"].as_i64().unwrap_or(0);
                        let left = begin.max(offset);
                        let right = end.min(offset + quote_units.len() as i64);
                        if left < right {
                            let mut clipped = entity.clone();
                            clipped["offset"] = json!(left - offset);
                            clipped["length"] = json!(right - left);
                            quoted_entities.push(clipped);
                        }
                    }
                    if !quoted_entities.is_empty() {
                        reply["quote_entities"] =
                            json!(build_entities(api, quote, &json!(quoted_entities)).await?);
                    }
                }
            }
            request["reply_to"] = reply;
        }
    }
    if let Some(when) = when {
        request["schedule_date"] = json!(when);
    }
    if let Some(repeat) = repeat {
        request["schedule_repeat_period"] = json!(repeat);
    }
    if let Some(identity) = a.get("send_as").filter(|v| !v.is_null()) {
        request["send_as"] = api.resolve_peer(identity).await?;
    }
    copy_optional(&mut request, a, &[("effect_id", "effect")]);
    let response = api
        .invoke(
            if edit {
                "messages.editMessage"
            } else {
                "messages.sendMessage"
            },
            request,
        )
        .await;
    match response {
        Err(error) if edit && error.to_string().contains("MESSAGE_NOT_MODIFIED") => {
            Ok(json!({"edited_message_id":mid,"not_modified":true}))
        }
        response => response,
    }
}
/// Scheduled ids live in a separate queue from sent message ids. Always bind
/// an edit to that queue and resend its date, including an overdue stored date.
async fn preserve_scheduled(
    api: &dyn Telegram,
    peer: &Value,
    mid: i64,
    args: &Value,
    request: &mut Value,
    when: Option<i64>,
    keep_body: bool,
) -> Result<()> {
    let history = api
        .invoke(
            "messages.getScheduledHistory",
            json!({"peer":peer,"hash":"0"}),
        )
        .await?;
    let current=result_messages(&history).into_iter().find(|message|message["id"].as_i64()==Some(mid)).ok_or_else(||anyhow::anyhow!("The message id is not in this chat's scheduled queue; a scheduled id is separate from a sent one"))?;
    if keep_body {
        let text = current["message"].as_str().unwrap_or("");
        ensure!(
            !text.is_empty() || current.get("media").is_some_and(|media| !media.is_null()),
            "The queued body is unreadable as plain text/media; cancel it and queue a new copy instead of replacing opaque rich content"
        );
        request["message"] = json!(text);
        request["entities"] = current.get("entities").cloned().unwrap_or(json!([]));
    }
    let date = when.or_else(|| current["date"].as_i64()).ok_or_else(|| {
        anyhow::anyhow!("The scheduled message has no date to preserve; supply when")
    })?;
    ensure!(
        (1..=i32::MAX as i64).contains(&date),
        "The stored scheduled date is invalid"
    );
    request["schedule_date"] = json!(date);
    if args.get("repeat").is_none_or(Value::is_null)
        && let Some(period) = current
            .get("schedule_repeat_period")
            .filter(|value| !value.is_null())
    {
        request["schedule_repeat_period"] = period.clone();
    }
    Ok(())
}
async fn wanted_peer(api: &dyn Telegram, p: &Value) -> Result<Value> {
    Ok(match p["_"].as_str() {
        Some("inputPeerChat") => json!({"_":"peerChat","chat_id":p["chat_id"]}),
        Some("inputPeerChannel") => json!({"_":"peerChannel","channel_id":p["channel_id"]}),
        Some("inputPeerUser") => json!({"_":"peerUser","user_id":p["user_id"]}),
        Some("inputPeerSelf") => {
            let users = api
                .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                .await?;
            json!({"_":"peerUser","user_id":users[0]["id"]})
        }
        _ => bail!("Could not bind deletion to the requested peer"),
    })
}
fn same_peer(a: &Value, b: &Value) -> bool {
    ["user_id", "chat_id", "channel_id"].iter().any(|key| {
        a.get(*key).filter(|v| !v.is_null()).is_some_and(|v| {
            v.to_string().trim_matches('"') == b[*key].to_string().trim_matches('"')
        })
    })
}
async fn delete(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let revoke = args::boolean(a, "revoke", name == "delete_messages_bulk")?;
    let message_ids = if name == "delete_chat_history" {
        vec![]
    } else if name == "delete_message" {
        vec![id(a, "message_id")?]
    } else {
        ids(a, "message_ids")?
    };
    let p = args::peer(api, a, "chat_id").await?;
    ensure!(
        channel(&p).is_none() || revoke,
        "A channel or supergroup has no private copy: revoke=false cannot delete messages there"
    );
    if name == "delete_chat_history" {
        let max_id = args::integer(a, "max_id", 0)?;
        ensure!(
            (0..=i32::MAX as i64).contains(&max_id),
            "max_id must be a nonnegative message id"
        );
        let request = json!({"peer":p,"max_id":max_id,"revoke":revoke,"just_clear":true});
        let started = std::time::Instant::now();
        let mut previous = None;
        for pass in 1..=20 {
            let remaining = std::time::Duration::from_secs(60).saturating_sub(started.elapsed());
            ensure!(
                !remaining.is_zero(),
                "History deletion reached its 60 second deadline; outcome is partial"
            );
            let result=tokio::time::timeout(remaining,api.invoke("messages.deleteHistory",request.clone())).await.map_err(|_|anyhow::anyhow!("History deletion timed out; outcome is unknown, inspect the chat before retrying"))??;
            let offset = result["offset"].as_i64().unwrap_or(0);
            if offset <= 0 {
                return Ok(json!({"deleted":true,"passes":pass,"result":result}));
            }
            ensure!(
                previous.is_none_or(|old| offset < old),
                "History deletion is incomplete: Telegram stopped reporting progress; remaining offset {offset}"
            );
            previous = Some(offset);
        }
        bail!("History deletion reached its 20 pass ceiling; outcome is partial")
    }
    if let Some(channel) = channel(&p) {
        return api
            .invoke(
                "channels.deleteMessages",
                json!({"channel":channel,"id":message_ids}),
            )
            .await;
    }
    // Non-channel message ids are account-global; inspect every peer before deleting any.
    let wanted = wanted_peer(api, &p).await?;
    let result=api.invoke("messages.getMessages",json!({"id":message_ids.iter().map(|mid|json!({"_":"inputMessageID","id":mid})).collect::<Vec<_>>()})).await?;
    let messages = result_messages(&result);
    ensure!(
        messages.iter().all(|m| same_peer(&m["peer_id"], &wanted)),
        "Refusing to delete: one or more message ids belong to another conversation"
    );
    let found: BTreeSet<i64> = messages.iter().filter_map(|m| m["id"].as_i64()).collect();
    let actual: Vec<i64> = message_ids
        .into_iter()
        .filter(|id| found.contains(id))
        .collect();
    if actual.is_empty() {
        return Ok(json!({"deleted":[],"already_missing":true}));
    }
    api.invoke(
        "messages.deleteMessages",
        json!({"id":actual,"revoke":revoke}),
    )
    .await
}
async fn forward(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let key = if name == "forward_messages" {
        "message_ids"
    } else {
        "message_id"
    };
    let mut message_ids = ids(a, key)?;
    let drop_author = name == "copy_message" || args::boolean(a, "drop_author", false)?;
    let drop_captions = args::boolean(a, "drop_captions", false)?;
    ensure!(
        !drop_captions || drop_author,
        "drop_captions requires drop_author=true or copy_message"
    );
    let when = schedule(a, "when")?;
    let repeat = repeat(a)?;
    let source = args::peer(api, a, "from_chat_id").await?;
    let destination = args::peer(api, a, "to_chat_id").await?;
    if name != "forward_messages" && args::boolean(a, "expand_album", true)? {
        let original = fetch(api, &source, message_ids[0]).await?;
        if original.get("grouped_id").is_some_and(|v| !v.is_null()) {
            let around = api
                .invoke(
                    "messages.getHistory",
                    history_request(source.clone(), 30, message_ids[0], -15),
                )
                .await?;
            let mut expanded: BTreeSet<i64> = message_ids.into_iter().collect();
            for message in result_messages(&around) {
                if message["grouped_id"] == original["grouped_id"]
                    && let Some(id) = message["id"].as_i64()
                {
                    expanded.insert(id);
                }
            }
            message_ids = expanded.into_iter().collect();
        }
    }
    let random_ids: Vec<Value> = message_ids.iter().map(|_| args::random_id()).collect();
    let mut req = json!({"from_peer":source,"to_peer":destination,"id":message_ids,"random_id":random_ids,"drop_author":drop_author,"drop_media_captions":drop_captions,"silent":args::boolean(a,"silent",false)?});
    if let Some(topic) = a.get("topic_id").filter(|v| !v.is_null()) {
        req["top_msg_id"] = json!(id(&json!({"topic_id":topic}), "topic_id")?);
    }
    if let Some(identity) = a.get("send_as").filter(|v| !v.is_null()) {
        req["send_as"] = api.resolve_peer(identity).await?;
    }
    if let Some(when) = when {
        req["schedule_date"] = json!(when);
    }
    if let Some(repeat) = repeat {
        req["schedule_repeat_period"] = json!(repeat);
    }
    api.invoke("messages.forwardMessages", req).await
}
async fn queue(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "get_drafts" {
        return api.invoke("messages.getAllDrafts", json!({})).await;
    }
    let p = args::peer(api, a, "chat_id").await?;
    match name {
        "list_scheduled_messages"|"get_scheduled_messages"=>api.invoke("messages.getScheduledHistory",json!({"peer":p,"hash":"0"})).await,
        "delete_scheduled_message"|"cancel_scheduled_message"=>api.invoke("messages.deleteScheduledMessages",json!({"peer":p,"id":ids(a,if name=="cancel_scheduled_message"{"message_id"}else{"message_ids"})?})).await,
        _=>{
            let message=if name=="clear_draft"{""}else{args::text(a,"message")?};
            let mut req=json!({"peer":p,"message":message,"no_webpage":args::boolean(a,"no_webpage",false)?});
            let reply_to=a.get("reply_to_msg_id").filter(|v|!v.is_null()).map(|_|id(a,"reply_to_msg_id")).transpose()?;
            if let Some(reply)=reply(a,reply_to)?{req["reply_to"]=reply;}
            api.invoke("messages.saveDraft",req).await
        }
    }
}
