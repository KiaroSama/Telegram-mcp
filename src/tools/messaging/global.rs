use super::*;
use std::{
    collections::VecDeque,
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tokio::time::{Instant, timeout_at};

struct Record {
    token: String,
    scope: Value,
    offset: Value,
    expiry: Instant,
}
static CURSORS: LazyLock<Mutex<VecDeque<Record>>> = LazyLock::new(Default::default);

fn save(scope: &Value, offset: &Value) -> Result<String> {
    let mut store = CURSORS
        .lock()
        .map_err(|_| anyhow::anyhow!("Search cursor store unavailable"))?;
    let now = Instant::now();
    store.retain(|r| r.expiry > now);
    if store.len() >= 1024 {
        store.pop_front();
    }
    let token = format!("{:032x}", rand::random::<u128>());
    store.push_back(Record {
        token: token.clone(),
        scope: scope.clone(),
        offset: offset.clone(),
        expiry: now + Duration::from_secs(900),
    });
    Ok(token)
}
fn load(token: &Value, scope: &Value) -> Result<Value> {
    let token = token
        .as_str()
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("Invalid search cursor")?;
    let store = CURSORS
        .lock()
        .map_err(|_| anyhow::anyhow!("Search cursor store unavailable"))?;
    store
        .iter()
        .find(|r| r.token == token && r.scope == *scope && r.expiry > Instant::now())
        .map(|r| r.offset.clone())
        .context("Unknown, expired or wrong-scope search cursor")
}
fn public(value: &mut Value) {
    match value {
        Value::Object(m) => {
            m.remove("access_hash");
            for v in m.values_mut() {
                public(v)
            }
        }
        Value::Array(xs) => xs.iter_mut().for_each(public),
        _ => (),
    }
}
fn decimal(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}
async fn next_peer(api: &dyn Telegram, response: &Value, peer: &Value) -> Result<Value> {
    let (key, table, kind, constructor) = match peer["_"].as_str() {
        Some("peerUser") => ("user_id", "users", "user", "inputPeerUser"),
        Some("peerChannel") => ("channel_id", "chats", "channel", "inputPeerChannel"),
        Some("peerChat") => ("chat_id", "chats", "chat", "inputPeerChat"),
        _ => anyhow::bail!("Telegram returned an invalid cursor peer"),
    };
    let bare = decimal(&peer[key])
        .filter(|n| *n > 0)
        .context("Invalid cursor peer ID")?;
    if let Some(entity) = response[table].as_array().and_then(|xs| {
        xs.iter()
            .find(|e| e["_"] == kind && decimal(&e["id"]) == Some(bare) && e["min"] != true)
    }) {
        if kind == "chat" {
            return Ok(json!({"_":constructor,key:bare.to_string()}));
        }
        if let Some(hash) = decimal(&entity["access_hash"]) {
            return Ok(
                json!({"_":constructor,key:bare.to_string(),"access_hash":hash.to_string()}),
            );
        }
    }
    let marked = match kind {
        "channel" => (-1_000_000_000_000i64)
            .checked_sub(bare)
            .context("Invalid cursor channel ID")?,
        "chat" => -bare,
        _ => bare,
    };
    let resolved = api
        .resolve_peer(&json!(marked.to_string()))
        .await
        .map_err(|_| anyhow::anyhow!("Cursor peer was not available"))?;
    ensure!(
        resolved["_"] == constructor
            && decimal(&resolved[key]) == Some(bare)
            && (kind == "chat" || decimal(&resolved["access_hash"]).is_some()),
        "Invalid resolved cursor peer"
    );
    Ok(resolved)
}
pub(super) async fn global(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let n = args::limit(a, "page_size", 20, 100)?;
    let page = args::integer(a, "page", 1)?;
    ensure!(page >= 1, "page must be positive");
    let mut remaining = (page - 1)
        .checked_mul(n)
        .context("Global page offset is too large")?;
    ensure!(
        remaining <= 2500 - n,
        "Legacy global pages are bounded to 2500 matches; use a cursor for deeper searches"
    );
    let query = a
        .get("query")
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().context("query must be text"))
        .transpose()?
        .unwrap_or("");
    if let Some(v) = a.get("media_type").filter(|v| !v.is_null()) {
        ensure!(v.is_string(), "media_type must be text");
    }
    let filter = media_filter(a)?;
    let scope = json!([api.cache_key(), a["account"], query, filter]);
    let mut offset = json!({"offset_peer":{"_":"inputPeerEmpty"},"offset_rate":0,"offset_id":0});
    if let Some(cursor) = a.get("cursor").filter(|v| !v.is_null()) {
        ensure!(page == 1, "Pass cursor with page=1");
        offset = load(cursor, &scope)?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = Vec::new();
    let mut empty = 0;
    for _ in 0..=50 {
        let mut req = offset.clone();
        for (k, v) in [
            ("q", json!(query)),
            ("filter", filter.clone()),
            ("min_date", json!(0)),
            ("max_date", json!(0)),
            (
                "limit",
                json!(if remaining > 0 { remaining.min(100) } else { n }),
            ),
        ] {
            req[k] = v;
        }
        let Ok(response) = timeout_at(deadline, api.invoke("messages.searchGlobal", req)).await
        else {
            break;
        };
        let mut response = response?;
        let batch = result_messages(&response)
            .into_iter()
            .filter(|m| m["_"] != "messageEmpty")
            .collect::<Vec<_>>();
        let rate = response
            .get("next_rate")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_i64()
                    .and_then(|i| i32::try_from(i).ok())
                    .context("Invalid search rate")
            })
            .transpose()?
            .unwrap_or(0);
        let mut next = offset.clone();
        next["offset_rate"] = json!(rate);
        if let Some(last) = batch.last() {
            empty = 0;
            next["offset_id"] = json!(id(last, "id")?);
            match timeout_at(deadline, next_peer(api, &response, &last["peer_id"])).await {
                Ok(p) => next["offset_peer"] = p?,
                Err(_) => {
                    response["partial"] = json!(true);
                    response["exhausted"] = json!(false);
                    response["has_more"] = json!(true);
                    response["next_cursor"] = Value::Null;
                    response["cursor_warning"] =
                        json!("Cursor resolution exceeded the search deadline");
                    if remaining > 0 {
                        response["fetched_messages"] = response["messages"].take();
                        response["messages"] = json!([]);
                        response["legacy_page_reached"] = json!(false);
                        response["legacy_page_advance_remaining"] = json!(remaining);
                    }
                    public(&mut response);
                    return Ok(response);
                }
            }
        } else {
            empty += 1;
        }
        let take = remaining == 0 && !batch.is_empty();
        remaining = remaining.saturating_sub(batch.len() as i64).max(0);
        let exhausted = batch.is_empty() && rate == 0;
        let stalled = offset == next || seen.contains(&next);
        if exhausted || stalled || take || empty >= 3 {
            response["messages"] = if take { json!(batch) } else { json!([]) };
            response["exhausted"] = json!(exhausted);
            response["has_more"] = json!(!exhausted);
            response["partial"] = json!(!exhausted && (stalled || empty >= 3));
            response["next_cursor"] = if exhausted || stalled || remaining > 0 {
                Value::Null
            } else {
                json!(save(&scope, &next)?)
            };
            if remaining > 0 {
                response["legacy_page_advance_remaining"] = json!(remaining);
            }
            public(&mut response);
            return Ok(response);
        }
        seen.push(offset);
        offset = next;
    }
    Ok(
        json!({"messages":[],"partial":true,"exhausted":false,"has_more":true,"next_cursor":if remaining==0{json!(save(&scope,&offset)?)}else{Value::Null},"legacy_page_advance_remaining":remaining}),
    )
}
