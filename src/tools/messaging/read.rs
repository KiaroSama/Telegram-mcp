use super::helpers::*;
mod general;
#[path = "global.rs"]
mod global;
mod projection;
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
        | "list_disappearing_media" => Some(run(api, name, a).await.map(|result| {
            let mut r = with_formatting(result);
            if let Some(returned) = r["messages"].as_array().map(Vec::len) {
                let (key, default, max) = match name {
                    "search_global" => ("page_size", 20, 100),
                    "get_messages" => ("page_size", 20, 200),
                    "get_history" => ("limit", 100, 200),
                    "inspect_messages" => ("limit", 10, 100),
                    _ => ("limit", 20, 200),
                };
                let requested = args::integer(a, key, default).unwrap_or(default);
                let effective = requested.min(max);
                if !r["metadata"].is_object() {
                    r["metadata"] = json!({});
                }
                r["metadata"]["requested_limit"] = json!(requested);
                r["metadata"]["effective_limit"] = json!(effective);
                r["metadata"]["limit_note"] = if effective < requested {
                    json!(format!("Requested {requested}; capped at {effective}"))
                } else {
                    Value::Null
                };
                r["metadata"]["has_more"] = json!(
                    r["has_more"]
                        .as_bool()
                        .unwrap_or(returned as i64 >= effective)
                );
            }
            r
        })),
        _ => None,
    }
}

fn with_formatting(mut result: Value) -> Value {
    projection::text(&mut result);
    describe_formatting(&mut result);
    if let Some(message) = result.get_mut("message").filter(|value| value.is_object()) {
        describe_formatting(message);
    }
    for key in ["messages", "fetched_messages"] {
        if let Some(messages) = result.get_mut(key).and_then(Value::as_array_mut) {
            for message in messages {
                describe_formatting(message);
            }
        }
    }
    result
}

fn describe_formatting(message: &mut Value) {
    // Media-only and rich-block results are not ordinary message records.
    if !message["message"].is_string() {
        return;
    }
    let kinds: std::collections::BTreeSet<_> = message["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entity| {
            Some(match entity["_"].as_str()? {
                "messageEntityBold" => "bold",
                "messageEntityItalic" => "italic",
                "messageEntityUnderline" => "underline",
                "messageEntityStrike" => "strikethrough",
                "messageEntitySpoiler" => "spoiler",
                "messageEntityCode" => "code",
                "messageEntityPre" => "pre",
                "messageEntityBlockquote" => "blockquote",
                "messageEntityTextUrl" => "text_url",
                "messageEntityCustomEmoji" => "custom_emoji",
                "messageEntityMention" => "mention",
                "messageEntityMentionName" => "mention_name",
                "messageEntityUrl" => "url",
                "messageEntityEmail" => "email",
                "messageEntityPhone" => "phone",
                "messageEntityHashtag" => "hashtag",
                "messageEntityBotCommand" => "bot_command",
                "messageEntityCashtag" => "cashtag",
                "messageEntityBankCard" => "bank_card",
                "messageEntityFormattedDate" => "formatted_date",
                "messageEntityDiffInsert" => "diff_insert",
                "messageEntityDiffReplace" => "diff_replace",
                "messageEntityDiffDelete" => "diff_delete",
                "messageEntityTonAddress" => "ton_address",
                "messageEntityUnknown" => "unknown",
                _ => return None,
            })
        })
        .collect();
    if !kinds.is_empty() {
        message["formatting"] = json!(kinds);
    }
}

async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "search_global" {
        return global::global(api, a).await.map(projection::link_page);
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
        return api
            .invoke("channels.searchPosts", req)
            .await
            .map(projection::link_page);
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
            let expected = requested_peer(api, &p).await?;
            let response = api
                .invoke("messages.getRichMessage", json!({"peer":p,"id":mid}))
                .await?;
            return super::rich_quote::view(&projection::link_page(response), &expected, mid);
        }
        let (mut msg, chat_names, username) = fetch_context(api, &p, mid).await?;
        projection::link(&mut msg, username.as_deref());
        if matches!(name, "get_media_info" | "get_media_details") {
            let mut details = json!({"message_id":mid,"media":msg["media"],"grouped_id":msg["grouped_id"],"entities":msg["entities"]});
            copy_optional(
                &mut details,
                &msg,
                &[("message", "message"), ("permalink", "permalink")],
            );
            return Ok(details);
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
                match crate::tools::visual::call(api,"get_telegram_screen",&json!({"verified_chat_names":chat_names,"max_dimension":a.get("max_dimension").cloned().unwrap_or(json!(1568))})).await.context("Screen route is unavailable")? {
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
        if a.get("topic_id").is_some_and(|v| !v.is_null()) && id(a, "topic_id")? == 1 {
            general::verify_forum(api, &p).await?;
        }
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
        if let Some(object) = req.as_object_mut() {
            object.remove("offset_date");
        }
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
        if topic == 1 && matches!(name, "get_messages" | "list_messages" | "search_messages") {
            let result = general::read(api, method, req, offset, bound).await?;
            if name == "get_messages" {
                return Ok(
                    json!({"page":page,"offset":offset,"requested_limit":a.get("page_size").cloned().unwrap_or(json!(20)),"effective_limit":bound,"returned":result["messages"].as_array().map_or(0, Vec::len),"has_more":result["has_more"],"scan_limit_reached":result["scan_limit_reached"],"messages":result["messages"]}),
                );
            }
            return Ok(result);
        }
        if method == "messages.search" {
            req["top_msg_id"] = json!(topic);
        } else {
            method = "messages.getReplies";
            req["msg_id"] = json!(topic);
        }
    }
    let mut response = projection::link_page(api.invoke(method, req.clone()).await?);
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
            response = projection::link_page(api.invoke(method, req.clone()).await?);
        }
        collected.truncate(bound as usize);
        response["messages"] = json!(collected);
    }
    if name == "list_disappearing_media" {
        return Ok(
            json!({"messages":result_messages(&response).into_iter().filter(|m|m["media"]["ttl_seconds"].as_i64().is_some_and(|ttl| ttl > 0)).collect::<Vec<_>>()}),
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
            Ok(value) => projection::link_page(value?),
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
            let actual_topic =
                crate::tools::local::upstream_events::message_topic_id(&message, true);
            if topic.is_some_and(|topic| {
                actual_topic != Some(topic) && !(topic > 1 && message["id"].as_i64() == Some(topic))
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
