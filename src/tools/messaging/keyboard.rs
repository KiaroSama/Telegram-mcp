use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

fn bindings() -> &'static Mutex<BTreeMap<String, (i64, Value)>> {
    static B: OnceLock<Mutex<BTreeMap<String, (i64, Value)>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(BTreeMap::new()))
}
fn account(a: &Value) -> String {
    a["account"]
        .as_str()
        .unwrap_or("default")
        .to_ascii_lowercase()
}
fn label(s: &str) -> String {
    s.chars().filter(|c|!c.is_control()&&!matches!(*c,'\u{202A}'..='\u{202E}'|'\u{2066}'..='\u{2069}'|'\u{200B}'|'\u{200C}'|'\u{200D}'|'\u{FEFF}')).take(256).collect()
}
fn buttons(message: &Value) -> Vec<Value> {
    message["reply_markup"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["buttons"].as_array().cloned().unwrap_or_default())
        .collect()
}
fn kind(button: &Value) -> &str {
    button["type"]["_"]
        .as_str()
        .or_else(|| button["_"].as_str())
        .unwrap_or("")
}
fn snapshot(
    api: &dyn Telegram,
    a: &Value,
    p: &Value,
    mid: i64,
    index: usize,
    button: &Value,
) -> Value {
    json!({"account":account(a),"session_key":api.cache_key(),"peer":p,"message_id":mid,"index":index,"button":button})
}
fn token(binding: Value) -> Result<String> {
    let mut cache = bindings()
        .lock()
        .map_err(|_| anyhow::anyhow!("Button binding store unavailable"))?;
    cache.retain(|_, (expiry, _)| *expiry > unix_time());
    ensure!(
        cache.len() < 10000,
        "Button binding capacity reached; wait for old bindings to expire"
    );
    let key = format!("{}:{}", args::random_id(), args::random_id()).replace('"', "");
    cache.insert(key.clone(), (unix_time() + 300, binding));
    Ok(key)
}
async fn keyboard_message(api: &dyn Telegram, p: &Value, a: &Value) -> Result<(Value, Vec<Value>)> {
    if a.get("message_id").is_some_and(|v| !v.is_null()) {
        let m = fetch(api, p, id(a, "message_id")?).await?;
        return Ok((m.clone(), vec![m]));
    }
    let result = api
        .invoke("messages.getHistory", history_request(p.clone(), 20, 0, 0))
        .await?;
    let recent = result_messages(&result);
    let message = recent
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No message was found"))?;
    Ok((message, recent))
}
pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "inspect_buttons" | "list_inline_buttons" => Some(inspect(api, a).await),
        "click_button" | "press_inline_button" => Some(click(api, name, a).await),
        "inline_query" | "send_inline_result" => Some(inline(api, name, a).await),
        "press_reply_button" | "answer_reply_button" => Some(reply_keyboard(api, name, a).await),
        _ => None,
    }
}
async fn inspect(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let p = args::peer(api, a, "chat_id").await?;
    let (message, recent) = keyboard_message(api, &p, a).await?;
    let mid = message["id"].as_i64().unwrap_or(0);
    let mut records = vec![];
    for (index, button) in buttons(&message).into_iter().enumerate() {
        let rendered = label(button["text"].as_str().unwrap_or(""));
        let button_kind = kind(&button);
        let mut row = json!({"index":index,"text":rendered,"kind":button_kind,"type":button["type"],"style":button["style"],"text_altered":button["text"].as_str()!=Some(rendered.as_str())});
        if button_kind == "inlineButtonTypeCallback"
            && !button["type"]["requires_password"]
                .as_bool()
                .unwrap_or(false)
        {
            row["press_token"] = json!(token(snapshot(api, a, &p, mid, index, &button))?);
            row["press_with"] = json!("click_button");
        }
        copy_optional(
            &mut row,
            &button["type"],
            &[
                ("url", "url"),
                ("copy_text", "copy_text"),
                ("query", "query"),
            ],
        );
        records.push(row);
    }
    let active = recent
        .iter()
        .find(|m| {
            matches!(
                m["reply_markup"]["_"].as_str(),
                Some("replyKeyboardMarkup" | "replyKeyboardHide" | "replyKeyboardForceReply")
            )
        })
        .cloned();
    Ok(
        json!({"message_id":mid,"buttons":records,"active_reply_keyboard":active.map(|m|json!({"message_id":m["id"],"markup":m["reply_markup"]}))}),
    )
}
async fn click(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let mid = id(a, "message_id")?;
    let index = args::integer(a, "button_index", -1)?;
    ensure!(
        index >= 0,
        "button_index is required and must be nonnegative"
    );
    let expected = args::text(
        a,
        if name == "press_inline_button" {
            "button_text"
        } else {
            "expect_text"
        },
    )?;
    let key = args::text(a, "press_token")?;
    let binding = {
        let cache = bindings()
            .lock()
            .map_err(|_| anyhow::anyhow!("Button binding store unavailable"))?;
        let (expiry, binding) = cache
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("Unknown press_token; inspect the buttons again"))?;
        ensure!(
            *expiry > unix_time(),
            "press_token expired; inspect the buttons again"
        );
        binding.clone()
    };
    ensure!(
        binding["account"] == account(a)
            && binding["message_id"] == mid
            && binding["index"] == index,
        "press_token belongs to a different account, message or index"
    );
    ensure!(
        binding["session_key"] == json!(api.cache_key()),
        "The account session changed; inspect the keyboard again"
    );
    let p = args::peer(api, a, "chat_id").await?;
    ensure!(binding["peer"] == p, "press_token belongs to another chat");
    let message = fetch(api, &p, mid).await?;
    let list = buttons(&message);
    let button = list
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("Button index no longer exists"))?;
    ensure!(
        binding["button"] == *button,
        "The keyboard changed; inspect it again before pressing"
    );
    ensure!(
        label(button["text"].as_str().unwrap_or("")) == expected,
        "The button label no longer matches expect_text"
    );
    ensure!(
        kind(button) == "inlineButtonTypeCallback",
        "Only callback buttons can be pressed; links and payments must use their own routes"
    );
    ensure!(
        !button["type"]["requires_password"]
            .as_bool()
            .unwrap_or(false),
        "This callback requires the account password and cannot be pressed through this tool"
    );
    bindings()
        .lock()
        .map_err(|_| anyhow::anyhow!("Button binding store unavailable"))?
        .remove(key);
    api.invoke(
        "messages.getBotCallbackAnswer",
        json!({"peer":p,"msg_id":mid,"data":button["type"]["data"]}),
    )
    .await
}
async fn inline(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "send_inline_result" {
        let handle = args::text(a, "result_id")?;
        let parts: Vec<&str> = handle.splitn(5, ':').collect();
        ensure!(
            parts.len() == 5 && parts[0] == "inline",
            "result_id must be a handle returned by inline_query"
        );
        ensure!(
            parts[1] == account(a),
            "Inline result belongs to another account"
        );
        ensure!(
            parts[2]
                .parse::<i64>()
                .is_ok_and(|expires| expires > unix_time()),
            "Inline result expired; repeat the query"
        );
        let query_id = parts[3]
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("Malformed inline query handle"))?;
        let p = args::peer(api, a, "chat_id").await?;
        let mut req = json!({"peer":p,"random_id":args::random_id(),"query_id":query_id.to_string(),"id":parts[4]});
        if let Some(reply) = reply(a, None)? {
            req["reply_to"] = reply;
        }
        return api.invoke("messages.sendInlineBotResult", req).await;
    }
    let query = args::text(a, "query")?;
    let bot = args::peer(api, a, "bot").await?;
    let user = input_user(&bot)?;
    let users = api.invoke("users.getUsers", json!({"id":[user]})).await?;
    ensure!(
        users[0]["bot"].as_bool().unwrap_or(false),
        "Only a bot can answer an inline query"
    );
    let p = api
        .resolve_peer(a.get("chat_id").unwrap_or(&json!("me")))
        .await?;
    let mut response=api.invoke("messages.getInlineBotResults",json!({"bot":input_user(&bot)?,"peer":p,"query":query,"offset":a["offset"].as_str().unwrap_or("")})).await?;
    let expiry = unix_time() + response["cache_time"].as_i64().unwrap_or(0).max(300);
    let query_id = response["query_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| response["query_id"].to_string());
    if let Some(rows) = response["results"].as_array_mut() {
        for row in rows {
            let result_id = row["id"].as_str().unwrap_or("");
            row["result_id"] = json!(format!(
                "inline:{}:{expiry}:{query_id}:{result_id}",
                account(a)
            ));
        }
    }
    Ok(response)
}
async fn reply_keyboard(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let index = args::integer(a, "button_index", -1)?;
    ensure!(index >= 0, "button_index must be nonnegative");
    let expected = args::text(a, "expect_text")?;
    let wait = a
        .get("wait_seconds")
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| anyhow::anyhow!("wait_seconds must be a number"))
        })
        .transpose()?
        .unwrap_or(10.0);
    ensure!(
        wait.is_finite() && (0.0..=30.0).contains(&wait),
        "wait_seconds must be within 0..30"
    );
    let p = args::peer(api, a, "chat_id").await?;
    let result = api
        .invoke("messages.getHistory", history_request(p.clone(), 20, 0, 0))
        .await?;
    let messages = result_messages(&result);
    let latest = messages.first().and_then(|m| m["id"].as_i64()).unwrap_or(0);
    let keyboard = messages
        .iter()
        .find(|m| {
            matches!(
                m["reply_markup"]["_"].as_str(),
                Some("replyKeyboardMarkup" | "replyKeyboardHide" | "replyKeyboardForceReply")
            )
        })
        .ok_or_else(|| anyhow::anyhow!("No active reply keyboard was found"))?;
    ensure!(
        keyboard["reply_markup"]["_"].as_str() == Some("replyKeyboardMarkup"),
        "The bot removed its reply keyboard or requested a free-text reply"
    );
    let list = buttons(keyboard);
    let button = list
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("button_index is outside the active keyboard"))?;
    ensure!(
        label(button["text"].as_str().unwrap_or("")) == expected,
        "expect_text does not match the current button"
    );
    let kind = kind(button);
    let raw = button["text"].as_str().unwrap_or("");
    ensure!(
        !list
            .iter()
            .any(|other| other["text"] == button["text"] && self::kind(other) != kind),
        "The same label belongs to different button kinds; sending it would be ambiguous"
    );
    let result = match kind {
        "buttonTypeDefault" => {
            ensure!(
                name == "press_reply_button",
                "Plain buttons use press_reply_button"
            );
            api.invoke(
                "messages.sendMessage",
                json!({"peer":p,"message":raw,"random_id":args::random_id()}),
            )
            .await?
        }
        "buttonTypeRequestGeoLocation" => {
            ensure!(
                name == "answer_reply_button",
                "Location sharing requires answer_reply_button"
            );
            let lat = a["latitude"]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("latitude is required"))?;
            let lon = a["longitude"]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("longitude is required"))?;
            ensure!(
                lat.is_finite()
                    && lon.is_finite()
                    && (-90.0..=90.0).contains(&lat)
                    && (-180.0..=180.0).contains(&lon),
                "Location coordinates are outside valid ranges"
            );
            api.invoke("messages.sendMedia",json!({"peer":p,"message":"","random_id":args::random_id(),"media":{"_":"inputMediaGeoPoint","geo_point":{"_":"inputGeoPoint","lat":lat,"long":lon}}})).await?
        }
        "buttonTypeRequestPhone" => {
            ensure!(
                name == "answer_reply_button",
                "Contact sharing requires answer_reply_button"
            );
            let users = api
                .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                .await?;
            let user = &users[0];
            ensure!(
                user["phone"].as_str().is_some_and(|s| !s.is_empty()),
                "The account has no available phone number"
            );
            api.invoke("messages.sendMedia",json!({"peer":p,"message":"","random_id":args::random_id(),"media":{"_":"inputMediaContact","phone_number":user["phone"],"first_name":user["first_name"].as_str().unwrap_or(""),"last_name":user["last_name"].as_str().unwrap_or(""),"vcard":""}})).await?
        }
        "buttonTypeRequestPeer" => {
            ensure!(
                name == "answer_reply_button",
                "Peer sharing requires answer_reply_button"
            );
            let selected = a["peers"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("peers must be an array"))?;
            let cap = button["type"]["max_quantity"].as_u64().unwrap_or(1) as usize;
            ensure!(
                !selected.is_empty() && selected.len() <= cap,
                "The number of selected peers is outside the button's limit"
            );
            let mut peers = vec![];
            for peer in selected {
                peers.push(api.resolve_peer(peer).await?);
            }
            api.invoke("messages.sendBotRequestedPeer",json!({"peer":p,"msg_id":keyboard["id"],"button_id":button["type"]["button_id"],"requested_peers":peers})).await?
        }
        "buttonTypeRequestPoll" => {
            ensure!(
                name == "answer_reply_button",
                "Poll requests require answer_reply_button"
            );
            let question = args::text(a, "question")?;
            ensure!(
                !question.is_empty() && question.encode_utf16().count() <= 300,
                "Poll question must contain 1 to 300 UTF-16 units"
            );
            let options = a["options"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("options must contain poll answer strings"))?;
            ensure!(
                (2..=10).contains(&options.len()),
                "Polls require 2 to 10 options"
            );
            let correct = a
                .get("correct_option_index")
                .filter(|v| !v.is_null())
                .map(|_| args::integer(a, "correct_option_index", -1))
                .transpose()?;
            let quiz = button["type"]
                .get("quiz")
                .and_then(Value::as_bool)
                .unwrap_or(correct.is_some());
            ensure!(
                !quiz || correct.is_some(),
                "A quiz poll needs correct_option_index"
            );
            ensure!(
                quiz || correct.is_none(),
                "This button requests a regular poll, not a quiz"
            );
            if let Some(correct) = correct {
                ensure!(
                    correct >= 0 && (correct as usize) < options.len(),
                    "correct_option_index is outside the options"
                );
            }
            let mut answers = vec![];
            for (index, option) in options.iter().enumerate() {
                let text = option
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Each poll option must be a string"))?;
                ensure!(
                    !text.is_empty() && text.encode_utf16().count() <= 100,
                    "Poll options must contain 1 to 100 UTF-16 units"
                );
                answers.push(json!({"_":"pollAnswer","text":{"_":"textWithEntities","text":text,"entities":[]},"option":[index]}));
            }
            let mut media = json!({"_":"inputMediaPoll","poll":{"_":"poll","id":"0","hash":"0","quiz":quiz,"question":{"_":"textWithEntities","text":question,"entities":[]},"answers":answers}});
            if let Some(correct) = correct {
                media["correct_answers"] = json!([correct]);
            }
            api.invoke(
                "messages.sendMedia",
                json!({"peer":p,"media":media,"message":"","random_id":args::random_id()}),
            )
            .await?
        }
        "buttonTypeSimpleWebView" => {
            ensure!(
                name == "answer_reply_button",
                "Web apps use answer_reply_button"
            );
            api.invoke(
                "messages.requestSimpleWebView",
                json!({"bot":input_user(&p)?,"url":button["type"]["url"],"platform":"desktop"}),
            )
            .await?
        }
        _ => bail!("This reply-button type requires a dedicated interaction and nothing was sent"),
    };
    if wait == 0.0 {
        return Ok(json!({"sent":result,"reply":null}));
    }
    let start = tokio::time::Instant::now();
    while start.elapsed().as_secs_f64() < wait {
        let history = api
            .invoke("messages.getHistory", history_request(p.clone(), 10, 0, 0))
            .await?;
        if let Some(message) = result_messages(&history).into_iter().find(|m| {
            m["id"].as_i64().unwrap_or(0) > latest && !m["out"].as_bool().unwrap_or(false)
        }) {
            return Ok(json!({"sent":result,"reply":message}));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Ok(json!({"sent":result,"reply":null,"timed_out_waiting_for_reply":true}))
}
