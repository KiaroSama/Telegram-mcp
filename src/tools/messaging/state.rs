use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "pin_message"
        | "unpin_message"
        | "unpin_all_messages"
        | "send_reaction"
        | "remove_reaction"
        | "get_message_reactions"
        | "mark_as_read"
        | "get_message_read_by"
        | "list_send_as"
        | "set_default_send_as"
        | "send_typing"
        | "list_calls" => Some(run(api, name, a).await),
        _ => None,
    }
}
async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "list_calls" {
        return calls(api, a).await;
    }
    let p = args::peer(api, a, "chat_id").await?;
    let mid = if matches!(
        name,
        "pin_message"
            | "unpin_message"
            | "send_reaction"
            | "remove_reaction"
            | "get_message_reactions"
            | "get_message_read_by"
    ) {
        id(a, "message_id")?
    } else {
        0
    };
    match name {
        "pin_message"|"unpin_message"=>api.invoke("messages.updatePinnedMessage",json!({"peer":p,"id":mid,"unpin":name=="unpin_message","silent":!args::boolean(a,"notify",false)?})).await,
        "unpin_all_messages"=>{
            let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(30);
            let mut last=Value::Null;
            for pass in 1..=20 {
                let outcome=tokio::time::timeout_at(deadline,api.invoke("messages.unpinAllMessages",json!({"peer":p}))).await;
                match outcome {
                    Ok(Ok(result))=>{
                        let offset=result["offset"].as_i64().ok_or_else(||anyhow::anyhow!("Unpin response omitted its continuation offset; inspect pins before retrying"))?;
                        last=result;
                        if offset==0{return Ok(json!({"completed":true,"passes":pass,"result":last}))}
                        ensure!(offset>0,"Unpin response has an invalid continuation offset");
                    },
                    _=>return Ok(json!({"completed":false,"passes":pass,"result":last,"outcome":"Unpin continuation failed or timed out; the current batch may have completed. Inspect pins before retrying."})),
                }
            }
            Ok(json!({"completed":false,"passes":20,"result":last,"outcome":"Unpin continuation reached the bounded batch ceiling; pins remain to inspect."}))
        },
        "send_reaction"|"remove_reaction"=>{
            let mut reactions=vec![];
            if name=="send_reaction"{
                if let Some(emoji)=a.get("emoji").filter(|v|!v.is_null()) {let emoji=emoji.as_str().ok_or_else(||anyhow::anyhow!("emoji must be a string"))?;ensure!(!emoji.is_empty(),"emoji cannot be empty");reactions.push(json!({"_":"reactionEmoji","emoticon":emoji}));}
                if let Some(raw)=a.get("custom_emoji_id").filter(|v|!v.is_null()) {
                    let ids=raw.as_array().cloned().unwrap_or(vec![raw.clone()]);
                    for document_id in ids {let s=document_id.as_str().map(str::to_owned).unwrap_or_else(||document_id.to_string());ensure!(s.parse::<i64>().is_ok_and(|n|n>0),"custom_emoji_id must contain positive 64-bit ids");reactions.push(json!({"_":"reactionCustomEmoji","document_id":s}));}
                }
                ensure!(!reactions.is_empty()&&reactions.len()<=3,"Supply 1 to 3 reactions");
            }
            api.invoke("messages.sendReaction",json!({"peer":p,"msg_id":mid,"reaction":reactions,"big":args::boolean(a,"big",false)?})).await
        },
        "get_message_reactions"=>{
            let mut req=json!({"peer":p,"id":mid,"limit":args::limit(a,"limit",50,100)?});
            copy_optional(&mut req,a,&[("offset","offset")]);
            api.invoke("messages.getMessageReactionsList",req).await
        },
        "get_message_read_by"=>api.invoke("messages.getMessageReadParticipants",json!({"peer":p,"msg_id":mid})).await,
        "mark_as_read"=>{
            if a.get("topic_id").is_some_and(|v|!v.is_null()){
                let topic=id(a,"topic_id")?;
                let result=api.invoke("messages.getForumTopicsByID",json!({"peer":p,"topics":[topic]})).await?;
                let top=result["topics"].as_array().and_then(|topics|topics.iter().find(|t|t["id"].as_i64()==Some(topic))).and_then(|t|t["top_message"].as_i64()).ok_or_else(||anyhow::anyhow!("Topic was not found"))?;
                api.invoke("messages.readDiscussion",json!({"peer":p,"msg_id":topic,"read_max_id":top})).await?;
                api.invoke("messages.readMentions",json!({"peer":p,"top_msg_id":topic})).await
            }else if let Some(channel)=channel(&p){api.invoke("channels.readHistory",json!({"channel":channel,"max_id":0})).await}else{api.invoke("messages.readHistory",json!({"peer":p,"max_id":0})).await}
        },
        "list_send_as"=>api.invoke("channels.getSendAs",json!({"peer":p})).await,
        "set_default_send_as"=>{let identity=args::peer(api,a,"send_as").await?;api.invoke("messages.saveDefaultSendAs",json!({"peer":p,"send_as":identity})).await},
        "send_typing"=>{
            let seconds=a.get("seconds").filter(|v|!v.is_null()).map(|v|v.as_f64().ok_or_else(||anyhow::anyhow!("seconds must be a number"))).transpose()?.unwrap_or(5.0);
            ensure!(seconds.is_finite()&&(0.0..=30.0).contains(&seconds),"seconds must be within 0..30");
            let mut req=json!({"peer":p,"action":{"_":"sendMessageTypingAction"}});
            if a.get("topic_id").is_some_and(|v|!v.is_null()){req["top_msg_id"]=json!(id(a,"topic_id")?);}
            let start=tokio::time::Instant::now();
            loop {api.invoke("messages.setTyping",req.clone()).await?;let remaining=std::time::Duration::from_secs_f64(seconds).saturating_sub(start.elapsed());if remaining.is_zero(){break}tokio::time::sleep(remaining.min(std::time::Duration::from_secs(4))).await;if start.elapsed().as_secs_f64()>=seconds{break}}
            req["action"]=json!({"_":"sendMessageCancelAction"});api.invoke("messages.setTyping",req).await
        },
        _=>unreachable!(),
    }
}
async fn calls(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let limit = args::integer(a, "limit", 50)?;
    ensure!(
        (1..=500).contains(&limit),
        "limit must be between 1 and 500"
    );
    let from = date(a, "from_date", false)?;
    let to = date(a, "to_date", true)?;
    ensure!(
        from == 0 || to == 0 || from < to,
        "from_date must be on or before to_date"
    );
    let peer = if let Some(chat) = a.get("chat_id").filter(|v| !v.is_null()) {
        api.resolve_peer(chat).await?
    } else {
        json!({"_":"inputPeerEmpty"})
    };
    let mut rows = vec![];
    let mut offset = 0;
    for _ in 0..20 {
        let wanted = (limit - rows.len() as i64).min(100);
        if wanted <= 0 {
            break;
        }
        let mut req = history_request(peer.clone(), wanted, offset, 0);
        req.as_object_mut().unwrap().remove("offset_date");
        req["q"] = json!("");
        req["filter"] = json!({"_":"inputMessagesFilterPhoneCalls"});
        req["min_date"] = json!(from);
        req["max_date"] = json!(to);
        let response = api.invoke("messages.search", req).await?;
        let messages = result_messages(&response);
        let old = offset;
        for message in &messages {
            offset = message["id"].as_i64().unwrap_or(offset);
            if message["action"]["_"].as_str() != Some("messageActionPhoneCall") {
                continue;
            }
            let time = message["date"].as_i64().unwrap_or(0);
            if from > 0 && time < from || to > 0 && time >= to {
                continue;
            }
            let identity = message["peer_id"]["user_id"].clone();
            let user = response["users"]
                .as_array()
                .and_then(|users| users.iter().find(|user| user["id"] == identity))
                .cloned()
                .unwrap_or(json!({"id":identity}));
            let name = [
                user["first_name"].as_str().unwrap_or(""),
                user["last_name"].as_str().unwrap_or(""),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            rows.push(json!({"message_id":message["id"],"peer":{"id":identity,"name":if name.is_empty(){"Deleted Account".to_owned()}else{name},"username":user["username"],"phone":user["phone"]},"date":time,"direction":if message["out"].as_bool().unwrap_or(false){"outgoing"}else{"incoming"},"duration":message["action"].get("duration").cloned().unwrap_or(json!(0)),"video":message["action"].get("video").cloned().unwrap_or(json!(false)),"end_reason":message["action"]["reason"]["_"].as_str().unwrap_or("unknown").trim_start_matches("phoneCallDiscardReason").to_ascii_lowercase()}));
        }
        if messages.len() < wanted as usize || offset == old {
            break;
        }
    }
    Ok(json!({"calls":rows,"count":rows.len()}))
}
