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
    let n = args::limit(a, "page_size", 20, 200)?;
    let page = args::integer(a, "page", 1)?;
    ensure!(page >= 1, "page must be at least 1");
    let cursor = a
        .get("cursor")
        .filter(|v| !v.is_null())
        .and_then(Value::as_str);
    ensure!(
        page == 1 || cursor.is_some(),
        "Global search requires the previous response's cursor after page 1"
    );
    let mut req = json!({"q":a.get("query").cloned().unwrap_or(json!("")),"filter":media_filter(a)?,"min_date":0,"max_date":0,"offset_rate":0,"offset_peer":{"_":"inputPeerEmpty"},"offset_id":0,"limit":n});
    if let Some(cursor) = cursor {
        let c: Value = serde_json::from_str(cursor)
            .map_err(|_| anyhow::anyhow!("Malformed global-search cursor"))?;
        ensure!(
            c["query"] == req["q"]
                && c["filter"] == req["filter"]
                && c["account"] == a.get("account").cloned().unwrap_or(Value::Null),
            "Search cursor belongs to another query, filter or account"
        );
        copy_optional(
            &mut req,
            &c,
            &[
                ("offset_rate", "offset_rate"),
                ("offset_peer", "offset_peer"),
                ("offset_id", "offset_id"),
            ],
        );
    }
    let mut response = api.invoke("messages.searchGlobal", req.clone()).await?;
    if let Some(last) = result_messages(&response).last() {
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
        let marked = if p.get("channel_id").is_some() {
            -1_000_000_000_000i64 - bare
        } else if p.get("chat_id").is_some() {
            -bare
        } else {
            bare
        };
        let peer = api.resolve_peer(&json!(marked.to_string())).await?;
        response["next_cursor"]=json!(json!({"query":req["q"],"filter":req["filter"],"account":a.get("account").cloned().unwrap_or(Value::Null),"offset_rate":response.get("next_rate").cloned().unwrap_or(json!(0)),"offset_peer":peer,"offset_id":last["id"]}).to_string());
    }
    Ok(response)
}
