use crate::{
    args,
    permalinks::{MessagePermalinkTarget, message_permalink},
    sanitize::project_entity_fields,
    text_fidelity::{DEFAULT_MAX_TEXT_CHARS, ProjectionOptions},
};
use serde_json::Value;
use std::collections::HashMap;

pub(super) fn text(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (text, entities) in [
                ("message", "entities"),
                ("quote_text", "quote_entities"),
                ("text", "entities"),
            ] {
                if text != "text" || fields.contains_key(entities) {
                    let options = ProjectionOptions {
                        max_chars: Some(DEFAULT_MAX_TEXT_CHARS),
                        ..ProjectionOptions::fidelity()
                    };
                    project_entity_fields(fields, text, entities, options, None);
                }
            }
            for child in fields
                .values_mut()
                .filter(|v| v.is_object() || v.is_array())
            {
                self::text(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(self::text),
        _ => (),
    }
}

fn peer_key(peer: &Value) -> Option<(&'static str, i64)> {
    let (kind, key) = match peer["_"].as_str()? {
        "peerChannel" => ("channel", "channel_id"),
        "peerUser" => ("user", "user_id"),
        "peerChat" => ("chat", "chat_id"),
        _ => return None,
    };
    Some((kind, args::integer(peer, key, 0).ok().filter(|v| *v != 0)?))
}

pub(super) fn link(message: &mut Value, username: Option<&str>) {
    let Some((kind, id)) = peer_key(&message["peer_id"]) else {
        return;
    };
    if let Some(link) = message_permalink(
        MessagePermalinkTarget {
            message_id: args::integer(message, "id", 0).unwrap_or(0),
            username,
            chat_id: Some(id),
            is_channel_or_supergroup: kind == "channel",
        },
        None,
    ) {
        message["permalink"] = Value::String(link);
    }
}

// Use each RPC page's own peer table before a paging or filtering wrapper discards it.
pub(super) fn link_page(mut response: Value) -> Value {
    let mut names = HashMap::new();
    for (table, kind) in [("users", "user"), ("chats", "chat"), ("chats", "channel")] {
        for e in response[table]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["_"] == kind)
        {
            if let (Ok(id), Some(name)) = (args::integer(e, "id", 0), e["username"].as_str()) {
                names.insert((kind, id), name.to_owned());
            }
        }
    }
    for key in ["messages", "fetched_messages"] {
        if let Some(rows) = response.get_mut(key).and_then(Value::as_array_mut) {
            for row in rows {
                let username = peer_key(&row["peer_id"]).and_then(|key| names.get(&key));
                if row["_"] != "messageEmpty" {
                    link(row, username.map(String::as_str));
                }
            }
        }
    }
    response
}
