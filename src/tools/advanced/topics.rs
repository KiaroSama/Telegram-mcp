use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "list_topics"
            | "enable_forum_topics"
            | "create_forum_topic"
            | "edit_forum_topic"
            | "delete_forum_topic"
            | "pin_forum_topic"
            | "reorder_pinned_topics"
            | "disable_forum_topics"
            | "upgrade_to_supergroup"
            | "list_forum_topic_icons"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
async fn forum(api: &dyn Telegram, args: &Value, enabled: bool) -> Result<(Value, Value)> {
    let p = a::peer(api, args, "chat_id").await?;
    let c = channel(&p)?;
    let result = api
        .invoke("channels.getChannels", json!({"id":[c]}))
        .await?;
    let e = result["chats"]
        .as_array()
        .and_then(|v| v.first())
        .ok_or_else(|| anyhow::anyhow!("Supergroup not found"))?;
    ensure!(
        e["megagroup"].as_bool() == Some(true),
        "The specified chat is not a supergroup"
    );
    if enabled {
        ensure!(
            e["forum"].as_bool() == Some(true),
            "Forum topics are not enabled; use enable_forum_topics first"
        );
    }
    Ok((p, e.clone()))
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    if name == "list_forum_topic_icons" {
        return api
            .invoke(
                "messages.getStickerSet",
                json!({"stickerset":{"_":"inputStickerSetEmojiDefaultTopicIcons"},"hash":0}),
            )
            .await;
    }
    if name == "upgrade_to_supergroup" {
        let p = a::peer(api, args, "chat_id").await?;
        ensure!(
            p["_"] == "inputPeerChat",
            "Only a basic group can be upgraded"
        );
        let mut result = api
            .invoke("messages.migrateChat", json!({"chat_id":p["chat_id"]}))
            .await?;
        if a::boolean(args, "enable_topics", false)? {
            let c=result["chats"].as_array().and_then(|v|v.iter().find(|c|c["_"]=="channel")).ok_or_else(||anyhow::anyhow!("Group was upgraded, but Telegram did not return its new channel; do not repeat the upgrade"))?;
            let input =
                json!({"_":"inputChannel","channel_id":c["id"],"access_hash":c["access_hash"]});
            match api
                .invoke(
                    "channels.toggleForum",
                    json!({"channel":input,"enabled":true,"tabs":true,"sidebar":false}),
                )
                .await
            {
                Ok(topics) => result["forum_updates"] = topics,
                Err(_) => {
                    result["warning"] = json!(
                        "Group was upgraded; enabling topics failed. Use enable_forum_topics with the new group ID; do not repeat the upgrade."
                    )
                }
            }
        }
        return Ok(result);
    }
    // Reject structural errors before looking up the peer or making any request.
    if name == "delete_forum_topic" {
        ensure!(
            a::integer(args, "topic_id", 0)? > 1,
            "The General topic (id 1) cannot be deleted"
        );
    }
    if name == "edit_forum_topic" {
        ensure!(
            ["title", "icon_emoji_id", "closed", "hidden"]
                .iter()
                .any(|k| args.get(*k).is_some_and(|v| !v.is_null())),
            "Nothing to change: give title, icon_emoji_id, closed or hidden"
        );
        if args.get("hidden").is_some_and(|v| !v.is_null()) {
            ensure!(
                a::integer(args, "topic_id", 0)? == 1,
                "Only the General topic can be hidden"
            );
        }
    }
    let (p, e) = forum(
        api,
        args,
        !matches!(name, "enable_forum_topics" | "disable_forum_topics"),
    )
    .await?;
    match name {
        "list_topics"=> {
            let n=a::limit(args,"limit",200,200)?;
            let offset=a::integer(args,"offset_topic",0)?;ensure!(offset>=0,"offset_topic must be nonnegative");
            let date=a::integer(args,"offset_date",0)?;let mid=a::integer(args,"offset_id",0)?;
            ensure!([offset,date,mid].iter().all(|v|(0..=i32::MAX as i64).contains(v)),"Topic cursor fields must fit nonnegative 32-bit integers");
            let mut params=json!({"peer":p,"offset_date":date,"offset_id":mid,"offset_topic":offset,"limit":n});
            if let Some(q)=args.get("search_query").filter(|v|!v.is_null()) {params["q"]=q.clone();}
            if args.get("icon_emoji_id").is_some_and(|v| !v.is_null()) {
                let icon = a::integer(args, "icon_emoji_id", 0)?;
                ensure!(icon > 0, "icon_emoji_id must be a positive 64-bit identifier");
                params["icon_emoji_id"] = json!(icon.to_string());
            }
            if args.get("closed").is_some_and(|v| !v.is_null()) {
                params["closed"] = json!(a::boolean(args, "closed", false)?);
            }
            let mut result=api.invoke("messages.getForumTopics",params).await?;
            let topics=result["topics"].as_array().cloned().unwrap_or_default();
            let last=topics.last();
            let next_topic=last.and_then(|v|id(&v["id"]).ok());
            let next_id=last.and_then(|v|id(&v["top_message"]).ok());
            let next_date=if result["order_by_create_date"]==true{last.and_then(|v|id(&v["date"]).ok())}else{
                result["messages"].as_array().and_then(|messages|messages.iter().find(|m|id(&m["id"]).ok()==next_id
                    && crate::server::access::marked_peer(&m["peer_id"])==crate::server::access::marked_peer(&p)))
                    .and_then(|m|id(&m["date"]).ok())
            };
            let cursor=next_topic.zip(next_id).zip(next_date).map(|((topic,id),date)|json!({"offset_topic":topic,"offset_id":id,"offset_date":date}));
            result["pagination"]=json!({"limit":n,"returned":topics.len(),"has_more":!topics.is_empty()&&result["count"].as_u64().is_none_or(|total|(topics.len()as u64)<total),
                "next_offset_topic":next_topic,"next_offset_id":next_id,"next_offset_date":next_date,"next_cursor":cursor,
                "note":"Continue with all three cursor fields. A short page can have more results; a final nonempty page may require one empty confirmation page."});
            Ok(result)
        }
        "enable_forum_topics"|"disable_forum_topics"=> {
            let enabled=name=="enable_forum_topics";
            if e["forum"].as_bool().unwrap_or(false)==enabled {return Ok(json!({"changed":false,"enabled":enabled}));}
            api.invoke("channels.toggleForum",json!({"channel":channel(&p)?,"enabled":enabled,"tabs":enabled&&a::boolean(args,"tabs",true)?,"sidebar":false})).await
        }
        "create_forum_topic"=> {
            let title=a::text(args,"title")?;nonempty(title,128,"Topic title")?;
            let mut params=json!({"peer":p,"title":title,"random_id":a::random_id()});
            copy_optional(args,&mut params,&["icon_color","icon_emoji_id"]);
            api.invoke("messages.createForumTopic",params).await
        }
        "edit_forum_topic"=> {
            let id=positive(a::required(args,"topic_id")?)?;
            if let Some(t)=args.get("title").and_then(Value::as_str) {nonempty(t,128,"Topic title")?;}
            let mut params=json!({"peer":p,"topic_id":id});copy_optional(args,&mut params,&["title","icon_emoji_id","closed","hidden"]);
            api.invoke("messages.editForumTopic",params).await
        }
        "delete_forum_topic"=> {
            let topic=positive(a::required(args,"topic_id")?)?;
            for round in 1..=50 {
                let r=api.invoke("messages.deleteTopicHistory",json!({"peer":p,"top_msg_id":topic})).await?;
                if r["offset"].as_i64().unwrap_or(0)==0 {return Ok(json!({"topic_id":topic,"deleted":true,"rounds":round}));}
            }
            Ok(json!({"topic_id":topic,"deleted":false,"rounds":50,"note":"Telegram still reports messages; call again."}))
        }
        "pin_forum_topic"=>api.invoke("messages.updatePinnedForumTopic",json!({"peer":p,"topic_id":positive(a::required(args,"topic_id")?)?,"pinned":a::boolean(args,"pinned",true)?})).await,
        "reorder_pinned_topics"=> {
            let order=ids(a::required(args,"topic_ids")?,100)?;unique(&order)?;
            api.invoke("messages.reorderPinnedForumTopics",json!({"peer":p,"order":order,"force":a::boolean(args,"force",false)?})).await
        }
        _=>bail!("Unknown forum operation"),
    }
}
