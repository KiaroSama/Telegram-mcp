use super::{channel, requested_peer, same_peer};
use crate::{backend::Telegram, tools::local::upstream_events::message_topic_id};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

pub(super) async fn verify_forum(api: &dyn Telegram, peer: &Value) -> Result<()> {
    let channel = channel(peer).context("General requires a Telegram forum")?;
    let expected = requested_peer(api, peer).await?;
    let result = api
        .invoke("channels.getChannels", json!({"id":[channel]}))
        .await?;
    ensure!(
        result["chats"]
            .as_array()
            .is_some_and(|chats| chats.iter().any(|chat| {
                chat["_"] == "channel"
                    && chat["forum"] == true
                    && same_peer(
                        &json!({"_":"peerChannel","channel_id":chat["id"]}),
                        &expected,
                    )
            })),
        "General requires a Telegram forum"
    );
    Ok(())
}

pub(super) async fn read(
    api: &dyn Telegram,
    method: &str,
    mut request: Value,
    skip: i64,
    limit: i64,
) -> Result<Value> {
    let peer = request["peer"].clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    timeout_at(deadline, verify_forum(api, &peer))
        .await
        .context("Forum validation timed out")??;
    let expected = requested_peer(api, &peer).await?;
    let mut rows = Vec::new();
    let mut remaining_skip = skip;
    let mut offset = 0;
    let mut incomplete = true;
    let mut answer = json!({"messages":[]});
    for _ in 0..25 {
        request["offset_id"] = json!(offset);
        request["add_offset"] = json!(0);
        request["limit"] = json!(100);
        let Ok(page) = timeout_at(deadline, api.invoke(method, request.clone())).await else {
            break;
        };
        answer = super::projection::link_page(page?);
        let page = answer["messages"]
            .as_array()
            .context("Telegram returned no message page")?;
        if page.is_empty() {
            incomplete = false;
            break;
        }
        for message in page {
            if !same_peer(&message["peer_id"], &expected)
                || message_topic_id(message, true) != Some(1)
                || message["_"] == "messageEmpty"
            {
                continue;
            }
            if remaining_skip > 0 {
                remaining_skip -= 1;
            } else {
                rows.push(message.clone());
                if rows.len() > limit as usize {
                    break;
                }
            }
        }
        if rows.len() > limit as usize {
            incomplete = false;
            break;
        }
        let next = page
            .last()
            .and_then(|message| message["id"].as_i64())
            .filter(|id| *id > 0);
        let Some(next) = next.filter(|id| offset == 0 || *id < offset) else {
            break;
        };
        offset = next;
    }
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    answer["messages"] = json!(rows);
    if let Some(object) = answer.as_object_mut() {
        object.remove("count");
    }
    answer["has_more"] = json!(has_more);
    answer["scan_limit_reached"] = json!(incomplete);
    Ok(answer)
}
