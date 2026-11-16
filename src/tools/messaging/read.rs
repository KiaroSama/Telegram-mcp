use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "get_messages"
        | "get_history"
        | "list_messages"
        | "get_message_context"
        | "get_pinned_messages"
        | "search_messages"
        | "search_global"
        | "search_posts"
        | "get_message_views"
        | "inspect_message"
        | "inspect_messages"
        | "read_rich_message"
        | "get_media_info"
        | "get_media_details"
        | "list_disappearing_media" => Some(run(api, name, a).await),
        _ => None,
    }
}
async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "search_global" {
        return global(api, a).await;
    }
    if name == "search_posts" {
        let n = args::limit(a, "limit", 20, 100)?;
        let mut req =
            json!({"offset_rate":0,"offset_peer":{"_":"inputPeerEmpty"},"offset_id":0,"limit":n});
        copy_optional(&mut req, a, &[("query", "query"), ("hashtag", "hashtag")]);
        ensure!(
            req["query"].as_str().is_some_and(|s| !s.is_empty())
                || req["hashtag"].as_str().is_some_and(|s| !s.is_empty()),
            "Provide query or hashtag"
        );
        return api.invoke("channels.searchPosts", req).await;
    }
    let bound = match name {
        "get_history" => args::limit(a, "limit", 100, 200)?,
        "get_messages" => args::limit(a, "page_size", 20, 200)?,
        "inspect_messages" => args::limit(a, "limit", 10, 100)?,
        _ => args::limit(a, "limit", 20, 200)?,
    };
    let page = args::integer(a, "page", 1)?;
    ensure!(page > 0, "page must be at least 1");
    let offset = (page - 1)
        .checked_mul(bound)
        .ok_or_else(|| anyhow::anyhow!("page offset is too large"))?;
    ensure!(
        offset <= 100000,
        "Paging beyond 100,000 records is unsupported; use date or search bounds"
    );
    let context = if name == "get_message_context" {
        {
            let context = args::integer(a, "context_size", 3)?;
            ensure!(
                (0..=100).contains(&context),
                "context_size must be between 0 and 100"
            );
            Some((id(a, "message_id")?, context))
        }
    } else {
        None
    };
    let date_from = date(a, "from_date", false)?;
    let date_to = date(a, "to_date", true)?;
    ensure!(
        date_to == 0 || date_from == 0 || date_from < date_to,
        "from_date must not be after to_date"
    );
    let filter = media_filter(a)?;
    let message_id = if matches!(
        name,
        "get_message_views"
            | "get_messages"
            | "get_history"
            | "list_messages"
            | "get_message_context"
            | "get_pinned_messages"
            | "search_messages"
            | "inspect_messages"
            | "list_disappearing_media"
    ) {
        None
    } else {
        Some(id(a, "message_id")?)
    };
    let views = if name == "get_message_views" {
        Some(ids(a, "message_ids")?)
    } else {
        None
    };
    let p = args::peer(api, a, "chat_id").await?;
    if let Some(ids) = views {
        return api
            .invoke(
                "messages.getMessagesViews",
                json!({"peer":p,"id":ids,"increment":false}),
            )
            .await;
    }
    if let Some(mid) = message_id {
        if name == "read_rich_message" {
            return api
                .invoke("messages.getRichMessage", json!({"peer":p,"id":mid}))
                .await;
        }
        let msg = fetch(api, &p, mid).await?;
        if matches!(name, "get_media_info" | "get_media_details") {
            return Ok(
                json!({"message_id":mid,"media":msg["media"],"grouped_id":msg["grouped_id"],"entities":msg["entities"]}),
            );
        }
        if name == "inspect_message" {
            let mut result = json!({"message":msg});
            let mut content = vec![];
            if args::boolean(a, "include_thumbnail", false)? {
                match super::media::call(api, "get_media_thumbnail", a)
                    .await
                    .context("Thumbnail route is unavailable")?
                {
                    Ok(preview) => {
                        content.extend(preview["content"].as_array().cloned().unwrap_or_default());
                        result["thumbnail"] = preview["metadata"].clone();
                    }
                    Err(_) => {
                        result["thumbnail_error"] =
                            json!("The media thumbnail could not be rendered")
                    }
                }
            }
            if args::boolean(a, "include_screen", false)? {
                match crate::tools::visual::call(api,"get_telegram_screen",&json!({"max_dimension":a.get("max_dimension").cloned().unwrap_or(json!(1568))})).await.context("Screen route is unavailable")? {
                    Ok(screen)=>{content.extend(screen["content"].as_array().cloned().unwrap_or_default());result["screen"]=screen.get("metadata").cloned().unwrap_or(screen);},
                    Err(_)=>result["screen_error"]=json!("A Telegram Desktop screen could not be captured"),
                }
            }
            if !content.is_empty() {
                result["content"] = json!(content);
            }
            return Ok(result);
        }
        return Ok(msg);
    }
    if name == "search_messages"
        && a["query"].as_str().unwrap_or("").is_empty()
        && a.get("from_user").is_some_and(|value| !value.is_null())
        && a["media_type"]
            .as_str()
            .is_some_and(|kind| !kind.is_empty())
    {
        return bounded_sender_media(api, &p, a, bound, date_from, date_to).await;
    }
    let mut req = history_request(p.clone(), bound, 0, offset);
    let mut method = "messages.getHistory";
    if let Some((mid, context)) = context {
        req = history_request(p.clone(), context * 2 + 1, mid, -context - 1);
    }
    if name == "inspect_messages" {
        req["offset_id"] = json!(args::integer(a, "offset_id", 0)?);
    }
    let search = name == "search_messages"
        || name == "get_pinned_messages"
        || a["search_query"].as_str().is_some_and(|s| !s.is_empty())
        || date_from > 0
        || date_to > 0;
    if search {
        method = "messages.search";
        req.as_object_mut().unwrap().remove("offset_date");
        req["q"] = a
            .get("query")
            .or_else(|| a.get("search_query"))
            .cloned()
            .unwrap_or(json!(""));
        req["filter"] = if name == "get_pinned_messages" {
            json!({"_":"inputMessagesFilterPinned"})
        } else {
            filter
        };
        req["min_date"] = json!(date_from);
        req["max_date"] = json!(date_to);
        if let Some(from) = a.get("from_user").filter(|v| !v.is_null()) {
            req["from_id"] = api.resolve_peer(from).await?;
        }
    }
    if a.get("topic_id").is_some_and(|v| !v.is_null()) {
        let topic = id(a, "topic_id")?;
        if method == "messages.search" {
            req["top_msg_id"] = json!(topic);
        } else {
            method = "messages.getReplies";
            req["msg_id"] = json!(topic);
        }
    }
    let mut response = api.invoke(method, req.clone()).await?;
    if name == "list_messages" && (date_from > 0 || date_to > 0) {
        let mut collected = Vec::new();
        let mut previous_offset = 0;
        for page_index in 0..20 {
            let batch = result_messages(&response);
            let mut oldest = 0;
            for message in &batch {
                let time = message["date"].as_i64().unwrap_or(0);
                oldest = if oldest == 0 { time } else { oldest.min(time) };
                if (date_from == 0 || time >= date_from) && (date_to == 0 || time < date_to) {
                    collected.push(message.clone());
                    if collected.len() >= bound as usize {
                        break;
                    }
                }
            }
            if collected.len() >= bound as usize
                || batch.len() < bound as usize
                || (date_from > 0 && oldest < date_from)
            {
                break;
            }
            let next = batch
                .last()
                .and_then(|message| message["id"].as_i64())
                .unwrap_or(0);
            if next == 0 || next == previous_offset {
                break;
            }
            previous_offset = next;
            req["offset_id"] = json!(next);
            req["add_offset"] = json!(0);
            req["limit"] = json!((bound - collected.len() as i64).max(1));
            if page_index == 19 {
                response["scan_limit_reached"] = json!(true);
                break;
            }
            response = api.invoke(method, req.clone()).await?;
        }
        collected.truncate(bound as usize);
        response["messages"] = json!(collected);
    }
    if name == "list_disappearing_media" {
        return Ok(
            json!({"messages":result_messages(&response).into_iter().filter(|m|m["media"].get("ttl_seconds").is_some()).collect::<Vec<_>>()}),
        );
    }
    if name == "get_messages" {
        let messages = result_messages(&response);
        return Ok(
            json!({"page":page,"offset":offset,"requested_limit":a.get("page_size").cloned().unwrap_or(json!(20)),"effective_limit":bound,"returned":messages.len(),"has_more":messages.len()==bound as usize,"messages":messages}),
        );
    }
    Ok(response)
}
async fn global(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let n = args::limit(a, "page_size", 20, 100)?;
    let page = args::integer(a, "page", 1)?;
    ensure!(page >= 1, "page must be at least 1");
    let cursor = a
        .get("cursor")
        .filter(|v| !v.is_null())
        .and_then(Value::as_str);
    let depth = (page - 1)
        .checked_mul(n)
        .ok_or_else(|| anyhow::anyhow!("Global page offset is too large"))?;
    ensure!(
        depth < 2500,
        "Legacy global pages are bounded to2500 matches; use a cursor for deeper searches"
    );
    let mut req = json!({"q":a.get("query").cloned().unwrap_or(json!("")),"filter":media_filter(a)?,"min_date":0,"max_date":0,"offset_rate":0,"offset_peer":{"_":"inputPeerEmpty"},"offset_id":0,"limit":n});
    if let Some(cursor) = cursor {
        apply_global_cursor(&mut req, a, cursor)?;
    }
    let mut remaining = if cursor.is_some() { 0 } else { depth };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    for _ in 0..=50 {
        req["limit"] = json!(if remaining > 0 {
            remaining.min(100)
        } else {
            n.min(2500 - depth)
        });
        let mut response = match tokio::time::timeout_at(
            deadline,
            api.invoke("messages.searchGlobal", req.clone()),
        )
        .await
        {
            Ok(response) => response?,
            Err(_) => {
                return Ok(
                    json!({"messages":[],"exhausted":false,"partial":true,"next_cursor":saved_global_cursor(a,&req),"legacy_page_advance_remaining":remaining}),
                );
            }
        };
        let batch = result_messages(&response);
        let next =
            match tokio::time::timeout_at(deadline, global_cursor(api, a, &req, &response)).await {
                Ok(next) => next?,
                Err(_) => {
                    response["partial"] = json!(true);
                    response["exhausted"] = json!(false);
                    response["next_cursor"] = Value::Null;
                    response["cursor_warning"] =
                        json!("Cursor resolution exceeded the search deadline");
                    if remaining > 0 {
                        // These matches precede the requested legacy page. Retain
                        // them without presenting them as that page's results.
                        response["fetched_messages"] = response["messages"].take();
                        response["messages"] = json!([]);
                        response["legacy_page_reached"] = json!(false);
                        response["legacy_page_advance_remaining"] = json!(remaining);
                    }
                    return Ok(response);
                }
            };
        if remaining == 0 || batch.is_empty() {
            response["exhausted"] = json!(batch.is_empty());
            response["partial"] = json!(false);
            if let Some(next) = next {
                response["next_cursor"] = json!(next);
            }
            return Ok(response);
        }
        remaining = remaining.saturating_sub(batch.len() as i64);
        let Some(next) = next else {
            return Ok(json!({"messages":[],"exhausted":true,"partial":false}));
        };
        let previous = (
            req["offset_rate"].clone(),
            req["offset_peer"].clone(),
            req["offset_id"].clone(),
        );
        apply_global_cursor(&mut req, a, &next)?;
        if previous
            == (
                req["offset_rate"].clone(),
                req["offset_peer"].clone(),
                req["offset_id"].clone(),
            )
        {
            return Ok(
                json!({"messages":[],"exhausted":false,"partial":true,"next_cursor":next,"legacy_page_advance_remaining":remaining}),
            );
        }
    }
    let next = saved_global_cursor(a, &req);
    Ok(
        json!({"messages":[],"exhausted":false,"partial":true,"next_cursor":next,"legacy_page_advance_remaining":remaining}),
    )
}
fn saved_global_cursor(a: &Value, req: &Value) -> String {
    json!({"query":req["q"],"filter":req["filter"],"account":a.get("account").cloned().unwrap_or(Value::Null),"offset_rate":req["offset_rate"],"offset_peer":req["offset_peer"],"offset_id":req["offset_id"]}).to_string()
}
fn apply_global_cursor(req: &mut Value, a: &Value, cursor: &str) -> Result<()> {
    let c: Value = serde_json::from_str(cursor)
        .map_err(|_| anyhow::anyhow!("Malformed global-search cursor"))?;
    ensure!(
        c["query"] == req["q"]
            && c["filter"] == req["filter"]
            && c["account"] == a.get("account").cloned().unwrap_or(Value::Null),
        "Search cursor belongs to another query, filter or account"
    );
    copy_optional(
        req,
        &c,
        &[
            ("offset_rate", "offset_rate"),
            ("offset_peer", "offset_peer"),
            ("offset_id", "offset_id"),
        ],
    );
    Ok(())
}
async fn global_cursor(
    api: &dyn Telegram,
    a: &Value,
    req: &Value,
    response: &Value,
) -> Result<Option<String>> {
    if let Some(last) = result_messages(response).last() {
        let p = last
            .get("peer_id")
            .ok_or_else(|| anyhow::anyhow!("Telegram omitted a cursor peer"))?;
        let identity = p
            .get("channel_id")
            .or_else(|| p.get("chat_id"))
            .or_else(|| p.get("user_id"))
            .ok_or_else(|| anyhow::anyhow!("Telegram returned an invalid cursor peer"))?;
        let bare = identity
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| identity.as_i64())
            .ok_or_else(|| anyhow::anyhow!("Cursor peer id is not a decimal integer"))?;
        ensure!(bare > 0, "Cursor peer id must be positive");
        let marked = if p.get("channel_id").is_some() {
            (-1_000_000_000_000i64)
                .checked_sub(bare)
                .context("Cursor channel id exceeds its bound")?
        } else if p.get("chat_id").is_some() {
            -bare
        } else {
            bare
        };
        let peer = api.resolve_peer(&json!(marked.to_string())).await?;
        return Ok(Some(json!({"query":req["q"],"filter":req["filter"],"account":a.get("account").cloned().unwrap_or(Value::Null),"offset_rate":response.get("next_rate").cloned().unwrap_or(json!(0)),"offset_peer":peer,"offset_id":last["id"]}).to_string()));
    }
    Ok(None)
}

async fn bounded_sender_media(
    api: &dyn Telegram,
    peer: &Value,
    a: &Value,
    limit: i64,
    from_date: i64,
    to_date: i64,
) -> Result<Value> {
    let sender = api.resolve_peer(&a["from_user"]).await?;
    let expected_sender = requested_peer(api, &sender).await?;
    let expected_chat = requested_peer(api, peer).await?;
    let topic = a
        .get("topic_id")
        .filter(|v| !v.is_null())
        .map(|_| id(a, "topic_id"))
        .transpose()?;
    let kind = a["media_type"].as_str().unwrap_or("");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut collected = vec![];
    let mut scanned = 0usize;
    let mut offset = 0;
    let mut exhausted = false;
    let mut stalled = false;
    let mut own_peer = (sender["_"] == "inputPeerSelf").then(|| expected_sender.clone());
    'scan: while scanned < 2500 && collected.len() < limit as usize {
        let wanted = (2500 - scanned).min(100);
        let mut request = history_request(peer.clone(), wanted as i64, offset, 0);
        let method = if let Some(topic) = topic.filter(|topic| *topic != 1) {
            request["msg_id"] = json!(topic);
            "messages.getReplies"
        } else {
            "messages.getHistory"
        };
        let response = match tokio::time::timeout_at(deadline, api.invoke(method, request)).await {
            Ok(value) => value?,
            Err(_) => break,
        };
        let messages = result_messages(&response);
        let count = messages.len();
        let mut next = offset;
        let before = scanned;
        for message in messages.into_iter().take(wanted) {
            scanned += 1;
            next = message["id"].as_i64().unwrap_or(next);
            if !same_peer(&message["peer_id"], &expected_chat) {
                continue;
            }
            let actual_topic = if message["reply_to"]["forum_topic"]
                .as_bool()
                .unwrap_or(false)
            {
                message["reply_to"]["reply_to_top_id"]
                    .as_i64()
                    .or_else(|| message["reply_to"]["reply_to_msg_id"].as_i64())
                    .unwrap_or(1)
            } else {
                1
            };
            if topic.is_some_and(|topic| {
                actual_topic != topic && !(topic > 1 && message["id"].as_i64() == Some(topic))
            }) {
                continue;
            }
            let sent_by = message
                .get("from_id")
                .filter(|value| !value.is_null())
                .or_else(|| {
                    (!message["out"].as_bool().unwrap_or(false)).then_some(&message["peer_id"])
                });
            let implicit_self = if sent_by.is_none()
                && expected_sender["_"] == "peerUser"
                && message["out"].as_bool().unwrap_or(false)
            {
                if own_peer.is_none() {
                    // Resolve self only when an outgoing message omits its
                    // sender, and reuse it for numeric and username selectors.
                    own_peer = Some(
                        match tokio::time::timeout_at(
                            deadline,
                            requested_peer(api, &json!({"_":"inputPeerSelf"})),
                        )
                        .await
                        {
                            Ok(peer) => peer?,
                            Err(_) => break 'scan,
                        },
                    );
                }
                own_peer
                    .as_ref()
                    .is_some_and(|peer| same_peer(peer, &expected_sender))
            } else {
                false
            };
            if !implicit_self && !sent_by.is_some_and(|actual| same_peer(actual, &expected_sender))
            {
                continue;
            }
            let date = message["date"].as_i64().unwrap_or(0);
            if from_date > 0 && date < from_date || to_date > 0 && date >= to_date {
                continue;
            }
            if matches_media(&message, kind) {
                collected.push(message);
                if collected.len() >= limit as usize {
                    break;
                }
            }
        }
        if count < wanted {
            exhausted = scanned - before == count;
            break;
        }
        if next <= 0 || next == offset {
            stalled = true;
            break;
        }
        offset = next;
    }
    Ok(
        json!({"messages":collected,"scanned":scanned,"scan_limit":2500,"exhausted":exhausted,"partial":stalled||!exhausted&&collected.len()<limit as usize,"effective_limit":limit,"requested_limit":a.get("limit").cloned().unwrap_or(json!(20))}),
    )
}
fn matches_media(message: &Value, kind: &str) -> bool {
    let media = &message["media"];
    let document = media.get("document").filter(|doc| doc["_"] == "document");
    let attributes = document.and_then(|d| d["attributes"].as_array());
    let has = |name: &str| attributes.is_some_and(|attrs| attrs.iter().any(|a| a["_"] == name));
    let audio = has("documentAttributeAudio");
    let video = has("documentAttributeVideo");
    let voice = attributes.is_some_and(|attrs| {
        attrs
            .iter()
            .any(|a| a["_"] == "documentAttributeAudio" && a["voice"].as_bool().unwrap_or(false))
    });
    let round = attributes.is_some_and(|attrs| {
        attrs.iter().any(|a| {
            a["_"] == "documentAttributeVideo" && a["round_message"].as_bool().unwrap_or(false)
        })
    });
    let photo = media["_"] == "messageMediaPhoto" || media.get("photo").is_some();
    match kind {
        "photos" => photo,
        "videos" => video && !round,
        "media" => photo || video && !round,
        "files" => {
            document.is_some()
                && !audio
                && !video
                && !has("documentAttributeAnimated")
                && !has("documentAttributeSticker")
        }
        "music" => audio && !voice,
        "voice" => voice,
        "round_voice" => voice || round,
        "round_video" => round,
        "gifs" => has("documentAttributeAnimated"),
        "geo" => matches!(
            media["_"].as_str(),
            Some("messageMediaGeo" | "messageMediaGeoLive" | "messageMediaVenue")
        ),
        "contacts" => media["_"] == "messageMediaContact",
        "phone_calls" => message["action"]["_"] == "messageActionPhoneCall",
        "chat_photos" => matches!(
            message["action"]["_"].as_str(),
            Some("messageActionChatEditPhoto" | "messageActionChatDeletePhoto")
        ),
        "pinned" => message["pinned"].as_bool().unwrap_or(false),
        "mentions" => message["mentioned"].as_bool().unwrap_or(false),
        "links" => {
            media["_"] == "messageMediaWebPage"
                || message["entities"].as_array().is_some_and(|entities| {
                    entities.iter().any(|e| {
                        matches!(
                            e["_"].as_str(),
                            Some("messageEntityUrl" | "messageEntityTextUrl")
                        )
                    })
                })
        }
        _ => false,
    }
}
