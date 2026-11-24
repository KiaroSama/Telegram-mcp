//! Resumable message streams and unread exports from current upstream.
use crate::server::access::{AccessPolicy, marked_peer};
use crate::{
    args,
    backend::Telegram,
    state::{EventQuery, State},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

pub const TOOL_NAMES: &[&str] = &[
    "wait_for_events",
    "get_messages_after",
    "export_unread_messages",
];

pub async fn call(api: &dyn Telegram, name: &str, input: &Value) -> Option<Result<Value>> {
    match name {
        "wait_for_events" => Some(wait_for_events(api, input).await),
        "get_messages_after" => Some(get_messages_after(api, input).await),
        "export_unread_messages" => Some(
            async {
                let state = State::global()?;
                super::unread::export_unread_with_state(api, &state, input).await
            }
            .await,
        ),
        _ => None,
    }
}

async fn wait_for_events(api: &dyn Telegram, input: &Value) -> Result<Value> {
    let state = State::global()?;
    let policy = AccessPolicy::from_environment()?;
    wait_for_events_with_state(api, &state, input, &policy).await
}

/// The server supplies its owned state and immutable policy snapshot before
/// projecting private username metadata from the selected event records.
pub async fn wait_for_events_with_state(
    api: &dyn Telegram,
    state: &State,
    input: &Value,
    policy: &AccessPolicy,
) -> Result<Value> {
    let timeout = input
        .get("timeout")
        .filter(|v| !v.is_null())
        .map(|value| value.as_f64().context("timeout must be a finite number"))
        .transpose()?
        .unwrap_or(50.);
    if !timeout.is_finite() || timeout < 0. {
        bail!("timeout must be a finite nonnegative number");
    }
    let after_seq = args::integer(input, "after_seq", 0)?;
    if after_seq < -1 {
        bail!("after_seq must be -1 or nonnegative");
    }
    let limit = args::limit(input, "limit", 100, 500)? as usize;
    let chat = if let Some(value) = input.get("chat_id").filter(|v| !v.is_null()) {
        let integer = value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()));
        if let Some(id) = integer {
            Some(id.to_string())
        } else {
            Some(
                marked_peer(&api.resolve_peer(value).await?)
                    .context("The selected chat has no marked identity")?,
            )
        }
    } else {
        None
    };
    let query = EventQuery {
        after_seq,
        boot: input["boot"].as_str().map(str::to_owned),
        account: input["account"].as_str().map(str::to_lowercase),
        chat_id: chat,
        timeout: Duration::from_secs_f64(timeout.min(50.)),
        limit,
    };
    state
        .wait_message_events(&query, &move |record| {
            let chat = crate::state::normalize_key(&record["chat_id"]);
            let topic = record["topic"]
                .as_i64()
                .or_else(|| record["topic"].as_str().and_then(|text| text.parse().ok()));
            policy.allows_message_event(&chat, record["_chat_username"].as_str(), topic)
        })
        .await
}

pub(crate) fn integer_id(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .context("Telegram message id must be an integer")
}

pub async fn get_messages_after(api: &dyn Telegram, input: &Value) -> Result<Value> {
    let limit = args::limit(input, "limit", 50, 100)?;
    let after = input
        .get("after_id")
        .filter(|value| !value.is_null())
        .map(integer_id)
        .transpose()?;
    if after.is_some_and(|id| id < 0 || id > i32::MAX as i64) {
        bail!("after_id must fit a nonnegative Telegram message id");
    }
    let topic = input
        .get("topic_id")
        .filter(|value| !value.is_null())
        .map(integer_id)
        .transpose()?;
    if topic.is_some_and(|id| id < 1 || id > i32::MAX as i64) {
        bail!("topic_id must be a positive Telegram message id");
    }
    AccessPolicy::from_environment()?
        .check("get_messages_after", input, api)
        .await?;
    let peer = api.resolve_peer(args::required(input, "chat_id")?).await?;
    let chat_id = marked_peer(&peer).context("The selected chat has no marked identity")?;
    if after == Some(i32::MAX as i64) {
        return Ok(json!({"chat_id":chat_id,"messages":[],"has_more":false,"next_after_id":after}));
    }
    let offset = after.map(|id| id + 1).unwrap_or(0);
    let add_offset = if after.is_some() { -limit } else { 0 };
    let mut request = json!({"peer":peer,"offset_id":offset,"offset_date":0,"add_offset":add_offset,"limit":limit,"max_id":0,"min_id":after.unwrap_or(0),"hash":"0"});
    let method = if let Some(topic) = topic {
        request["msg_id"] = json!(topic);
        "messages.getReplies"
    } else {
        "messages.getHistory"
    };
    let result = api.invoke(method, request).await?;
    let fetched = result["messages"]
        .as_array()
        .context("Telegram did not return a message page")?
        .clone();
    let next = fetched
        .iter()
        .filter_map(|message| integer_id(&message["id"]).ok())
        .max()
        .or(after);
    let mut messages = fetched
        .iter()
        .filter(|message| message["_"] != "messageEmpty")
        .filter(|message| {
            after.is_none_or(|id| integer_id(&message["id"]).is_ok_and(|candidate| candidate > id))
        })
        .filter(|message| topic.is_none_or(|topic| message_topic_id(message, true) == Some(topic)))
        .map(|message| {
            let mut record = message_record(message, &result, &peer, true);
            if let Some(topic) = topic {
                record["topic"] = json!(topic);
            }
            record
        })
        .collect::<Vec<_>>();
    messages.sort_by_key(|message| integer_id(&message["id"]).unwrap_or(0));
    let mut payload = json!({"chat_id":chat_id,"chat_type":chat_kind(&peer,&result),"messages":messages,"has_more":fetched.len() as i64>=limit,"next_after_id":next,"topic_id":topic,"untrusted_content":true});
    if is_forum(&peer, &result) {
        let topics = messages
            .iter()
            .filter_map(|record| record["topic"].as_i64())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !topics.is_empty() {
            match tokio::time::timeout(
                Duration::from_secs(5),
                api.invoke(
                    "messages.getForumTopicsByID",
                    json!({"peer":peer,"topics":topics}),
                ),
            )
            .await
            {
                Ok(Ok(response)) => {
                    let mut names = serde_json::Map::new();
                    for topic in response["topics"].as_array().into_iter().flatten() {
                        if let (Ok(id), Some(title)) =
                            (integer_id(&topic["id"]), topic["title"].as_str())
                            && topics.contains(&id)
                        {
                            names.insert(
                                id.to_string(),
                                json!(title.chars().take(256).collect::<String>()),
                            );
                        }
                    }
                    if !names.is_empty() {
                        payload["topics"] = Value::Object(names);
                    }
                }
                _ => {
                    payload["topic_titles_unavailable"] = json!(true);
                }
            }
        }
    }
    Ok(payload)
}

fn is_forum(peer: &Value, result: &Value) -> bool {
    result["chats"].as_array().is_some_and(|chats| {
        chats.iter().any(|chat| {
            crate::state::normalize_key(&chat["id"])
                == crate::state::normalize_key(&peer["channel_id"])
                && chat["forum"] == json!(true)
        })
    })
}

pub fn message_topic_id(message: &Value, forum: bool) -> Option<i64> {
    if !forum {
        return None;
    }
    if message["action"]["_"] == "messageActionTopicCreate" {
        return integer_id(&message["id"]).ok();
    }
    if message["reply_to"]["forum_topic"] == json!(true) {
        integer_id(&message["reply_to"]["reply_to_top_id"])
            .ok()
            .or_else(|| integer_id(&message["reply_to"]["reply_to_msg_id"]).ok())
    } else {
        Some(1)
    }
}

pub fn message_reply_id(message: &Value) -> Option<i64> {
    let reply = integer_id(&message["reply_to"]["reply_to_msg_id"]).ok()?;
    if message["reply_to"]["forum_topic"] == json!(true) {
        let top = integer_id(&message["reply_to"]["reply_to_top_id"]).ok();
        if top.is_none() || top == Some(reply) {
            return None;
        }
    }
    Some(reply)
}

fn chat_kind(peer: &Value, result: &Value) -> &'static str {
    match peer["_"].as_str().unwrap_or("") {
        "inputPeerChat" => "group",
        "inputPeerChannel" => {
            if result["chats"].as_array().is_some_and(|chats| {
                chats.iter().any(|chat| {
                    crate::state::normalize_key(&chat["id"])
                        == crate::state::normalize_key(&peer["channel_id"])
                        && chat["megagroup"] == json!(true)
                })
            }) {
                "supergroup"
            } else {
                "channel"
            }
        }
        _ => "private",
    }
}

pub(crate) fn message_record(message: &Value, result: &Value, peer: &Value, media: bool) -> Value {
    let sender_id = marked_peer(&message["from_id"]);
    let sender = result["users"]
        .as_array()
        .and_then(|users| {
            users.iter().find(|user| {
                sender_id
                    .as_ref()
                    .is_some_and(|id| crate::state::normalize_key(&user["id"]) == *id)
            })
        })
        .map(|user| {
            let name = [user["first_name"].as_str(), user["last_name"].as_str()]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if name.is_empty() {
                "Deleted Account".to_owned()
            } else {
                name
            }
        });
    let mut record = message.clone();
    if !record.is_object() {
        return json!({"error":"invalid_message_record"});
    }
    record["text"] = message.get("message").cloned().unwrap_or(json!(""));
    record["chat_id"] = json!(marked_peer(peer));
    record["sender_id"] = json!(sender_id);
    record["sender"] = json!(sender);
    if let Some(timestamp) = message["date"]
        .as_i64()
        .or_else(|| message["date"].as_str().and_then(|date| date.parse().ok()))
        && let Some(date) = chrono::DateTime::from_timestamp(timestamp, 0)
    {
        record["date"] = json!(date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    if let Some(header) = message.get("reply_to").filter(|header| !header.is_null()) {
        record["reply_to_header"] = header.clone();
    }
    record["reply_to"] = json!(message_reply_id(message));
    if let Some(topic) = message_topic_id(message, is_forum(peer, result)) {
        record["topic"] = json!(topic);
    }
    let out = message["out"].as_bool().unwrap_or(false);
    record["out"] = json!(out);
    record["addressed"] = json!(
        !out && (chat_kind(peer, result) == "private"
            || message["mentioned"].as_bool().unwrap_or(false))
    );
    if !media {
        record
            .as_object_mut()
            .expect("Message object validated")
            .remove("media");
    }
    record
}
