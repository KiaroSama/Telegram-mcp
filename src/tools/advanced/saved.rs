use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "list_saved_dialogs"
            | "get_saved_history"
            | "list_saved_tags"
            | "name_saved_tag"
            | "list_quick_replies"
            | "send_quick_reply"
            | "add_quick_reply"
            | "read_quick_reply"
            | "edit_quick_reply"
            | "rename_quick_reply"
            | "delete_quick_reply"
            | "inspect_sticker_set"
            | "suggest_sticker_set_name"
            | "add_sticker_to_set"
            | "remove_sticker_from_set"
            | "move_sticker_in_set"
            | "install_sticker_set"
            | "uninstall_sticker_set"
            | "get_sticker_sets"
            | "list_saved_gifs"
            | "save_gif"
            | "unsave_gif"
            | "get_gif_search"
            | "send_gif"
            | "open_mini_app"
            | "translate"
            | "get_stars_receipt"
            | "list_message_effects"
            | "get_message_effect"
    ) {
        return None;
    }
    Some(match name {
        "get_gif_search" | "send_gif" => gif(api, name, args).await,
        "open_mini_app" => mini_app(api, args).await,
        "translate" => translate(api, args).await,
        "get_stars_receipt" => stars(api, args).await,
        "list_message_effects" | "get_message_effect" => super::effects::run(api, name, args).await,
        _ => run(api, name, args).await,
    })
}
fn sticker_ref(args: &Value) -> Result<Value> {
    if let Some(s) = args
        .get("short_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Ok(json!({"_":"inputStickerSetShortName","short_name":s}));
    }
    let sid = positive(a::required(args, "set_id")?)?;
    let hash = id(a::required(args, "access_hash")?)?;
    Ok(json!({"_":"inputStickerSetID","id":sid.to_string(),"access_hash":hash.to_string()}))
}
async fn replies(api: &dyn Telegram) -> Result<Value> {
    api.invoke("messages.getQuickReplies", json!({"hash":0}))
        .await
}
async fn find_reply(api: &dyn Telegram, sid: i64) -> Result<Value> {
    let r = replies(api).await?;
    r["quick_replies"]
        .as_array()
        .and_then(|v| v.iter().find(|v| id(&v["shortcut_id"]).ok() == Some(sid)))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Shortcut not found; use list_quick_replies"))
}
async fn reply_messages(api: &dyn Telegram, sid: i64) -> Result<Value> {
    api.invoke(
        "messages.getQuickReplyMessages",
        json!({"shortcut_id":sid,"hash":0}),
    )
    .await
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    match name {
        "list_saved_dialogs"=>api.invoke("messages.getSavedDialogs",json!({"offset_date":0,"offset_id":0,"offset_peer":{"_":"inputPeerSelf"},"limit":a::limit(args,"limit",50,100)?,"hash":0})).await,
        "get_saved_history"=>api.invoke("messages.getSavedHistory",json!({"peer":a::peer(api,args,"peer_id").await?,"offset_id":0,"offset_date":0,"add_offset":0,"limit":a::limit(args,"limit",30,100)?,"max_id":0,"min_id":0,"hash":0})).await,
        "list_saved_tags"=>api.invoke("messages.getSavedReactionTags",json!({"hash":0})).await,
        "name_saved_tag"=>{let mut p=json!({"reaction":reaction(args,false)?});if let Some(title)=args.get("title").and_then(Value::as_str).filter(|s|!s.is_empty()){p["title"]=json!(title);}api.invoke("messages.updateSavedReactionTag",p).await},
        "list_quick_replies"=>replies(api).await,
        "read_quick_reply"=>reply_messages(api,positive(a::required(args,"shortcut_id")?)?).await,
        "add_quick_reply"=>{
            let s=a::text(args,"shortcut")?.trim().trim_start_matches('/');nonempty(s,32,"Shortcut")?;
            let body=a::text(args,"text")?;nonempty(body,4096,"Quick reply text")?;
            let before=replies(api).await?;
            let exists=before["quick_replies"].as_array().is_some_and(|r|r.iter().any(|r|r["shortcut"]==s));
            let mut p=json!({"peer":{"_":"inputPeerSelf"},"message":body,"random_id":a::random_id(),"quick_reply_shortcut":{"_":"inputQuickReplyShortcut","shortcut":s}});
            if let Some(e)=args.get("entities").filter(|v|!v.is_null()) {p["entities"]=super::common::entities(api,e,body).await?;}
            let sent=api.invoke("messages.sendMessage",p).await?;
            let mut output=json!({"created":!exists,"shortcut":s,"updates":sent});
            match replies(api).await {
                Ok(catalogue)=>output["catalogue"]=catalogue,
                Err(_)=>output["warning"]=json!("Quick reply was saved, but catalogue refresh failed. Do not repeat the add; use list_quick_replies."),
            }
            Ok(output)
        }
        "edit_quick_reply"=>{
            let sid=positive(a::required(args,"shortcut_id")?)?;let mid=positive(a::required(args,"message_id")?)?;
            let held=reply_messages(api,sid).await?;
            ensure!(held["messages"].as_array().is_some_and(|v|v.iter().any(|v|id(&v["id"]).ok()==Some(mid))),"Message does not belong to this shortcut; use read_quick_reply");
            let body=a::text(args,"message")?;nonempty(body,4096,"Message")?;
            let mut p=json!({"peer":{"_":"inputPeerSelf"},"id":mid,"message":body,"quick_reply_shortcut_id":sid});
            if let Some(e)=args.get("entities").filter(|v|!v.is_null()) {p["entities"]=super::common::entities(api,e,body).await?;}
            api.invoke("messages.editMessage",p).await
        }
        "rename_quick_reply"=>{
            let sid=positive(a::required(args,"shortcut_id")?)?;let s=a::text(args,"shortcut")?.trim().trim_start_matches('/');nonempty(s,32,"Shortcut")?;
            find_reply(api,sid).await?;
            api.invoke("messages.editQuickReplyShortcut",json!({"shortcut_id":sid,"shortcut":s})).await
        }
        "delete_quick_reply"=>{
            let sid=positive(a::required(args,"shortcut_id")?)?;find_reply(api,sid).await?;
            if let Some(v)=args.get("message_ids").filter(|v|!v.is_null()) {
                let wanted=ids(v,100)?;unique(&wanted)?;
                let held=reply_messages(api,sid).await?;
                ensure!(wanted.iter().all(|mid|held["messages"].as_array().is_some_and(|v|v.iter().any(|v|id(&v["id"]).ok()==Some(*mid)))),"Some IDs do not belong to this shortcut");
                api.invoke("messages.deleteQuickReplyMessages",json!({"shortcut_id":sid,"id":wanted})).await
            }else{api.invoke("messages.deleteQuickReplyShortcut",json!({"shortcut_id":sid})).await}
        }
        "send_quick_reply"=>{
            let sid=positive(a::required(args,"shortcut_id")?)?;
            let wanted=if let Some(v)=args.get("message_ids").filter(|v|!v.is_null()){ids(v,100)?}else{
                let r=reply_messages(api,sid).await?;
                r["messages"].as_array().map(|v|v.iter().filter_map(|v|id(&v["id"]).ok()).collect()).unwrap_or_default()
            };ensure!(!wanted.is_empty(),"Shortcut contains no messages");unique(&wanted)?;
            let random:Vec<_>=wanted.iter().map(|_|a::random_id()).collect();
            api.invoke("messages.sendQuickReplyMessages",json!({"peer":a::peer(api,args,"chat_id").await?,"shortcut_id":sid,"id":wanted,"random_id":random})).await
        }
        "inspect_sticker_set"=>{
            let result=api.invoke("messages.getStickerSet",json!({"stickerset":sticker_ref(args)?,"hash":0})).await?;
            super::sticker_refs::remember(api,&result)?;Ok(result)
        },
        "suggest_sticker_set_name"=>{
            let title=a::text(args,"title")?;nonempty(title,64,"Sticker set title")?;
            let result=api.invoke("stickers.suggestShortName",json!({"title":title})).await?;
            let name=result["short_name"].as_str().ok_or_else(||anyhow::anyhow!("Telegram returned no sticker-set name"))?;
            let available=api.invoke("stickers.checkShortName",json!({"short_name":name})).await?;
            Ok(json!({"short_name":name,"available":available}))
        }
        "add_sticker_to_set"=>{
            let r=sticker_ref(args)?;let emoji=a::text(args,"emoji")?;nonempty(emoji,64,"Sticker emoji")?;
            if let Some(expected)=args.get("expected_count").filter(|v|!v.is_null()){
                let current=api.invoke("messages.getStickerSet",json!({"stickerset":r,"hash":0})).await?;
                ensure!(id(expected)?==id(&current["set"]["count"])?,"Sticker-set count changed; inspect the set before adding");
            }
            api.invoke("stickers.addStickerToSet",json!({"stickerset":r,"sticker":{"_":"inputStickerSetItem","document":super::sticker_refs::resolve(api,args).await?,"emoji":emoji}})).await
        }
        "remove_sticker_from_set"=>api.invoke("stickers.removeStickerFromSet",json!({"sticker":super::sticker_refs::resolve(api,args).await?})).await,
        "move_sticker_in_set"=>{
            let position=a::integer(args,"position",-1)?;ensure!(position>=0,"position must be nonnegative");
            api.invoke("stickers.changeStickerPosition",json!({"sticker":super::sticker_refs::resolve(api,args).await?,"position":position})).await
        }
        "install_sticker_set"=>api.invoke("messages.installStickerSet",json!({"stickerset":sticker_ref(args)?,"archived":a::boolean(args,"archived",false)?})).await,
        "uninstall_sticker_set"=>api.invoke("messages.uninstallStickerSet",json!({"stickerset":sticker_ref(args)?})).await,
        "get_sticker_sets"=>{
            let kind=string(args,"kind","stickers")?;
            if kind=="both"{let stickers=api.invoke("messages.getAllStickers",json!({"hash":0})).await?;let emoji=api.invoke("messages.getEmojiStickers",json!({"hash":0})).await?;let mut sets=stickers["sets"].as_array().cloned().unwrap_or_default();sets.extend(emoji["sets"].as_array().cloned().unwrap_or_default());return Ok(json!({"sets":sets,"kind":"both"}));}
            let method=match kind{"stickers"=>"messages.getAllStickers","masks"=>"messages.getMaskStickers","emoji"=>"messages.getEmojiStickers",_=>bail!("kind must be stickers, masks, emoji or both")};
            api.invoke(method,json!({"hash":0})).await
        }
        "list_saved_gifs"=>api.invoke("messages.getSavedGifs",json!({"hash":0})).await,
        "save_gif"=>{
            let p=a::peer(api,args,"chat_id").await?;let msg=message(api,&p,positive(a::required(args,"message_id")?)?).await?;
            let d=&msg["media"]["document"];
            ensure!(d["attributes"].as_array().is_some_and(|v|v.iter().any(|v|v["_"]=="documentAttributeAnimated")),"Message is not an animation or GIF");
            api.invoke("messages.saveGif",json!({"id":document(d)?,"unsave":false})).await
        }
        "unsave_gif"=>{
            let did=positive(a::required(args,"document_id")?)?;
            let list=api.invoke("messages.getSavedGifs",json!({"hash":0})).await?;
            let d=list["gifs"].as_array().and_then(|v|v.iter().find(|v|id(&v["id"]).ok()==Some(did))).ok_or_else(||anyhow::anyhow!("GIF is not in this account's saved GIFs"))?;
            api.invoke("messages.saveGif",json!({"id":document(d)?,"unsave":true})).await
        }
        _=>bail!("Unknown saved-media operation"),
    }
}
async fn gif(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let account = string(args, "account", "default")?.to_lowercase();
    if name == "get_gif_search" {
        let n = a::limit(args, "limit", 10, 50)?;
        let bot = user(&api.resolve_peer(&json!("gif")).await?)?;
        let result=api.invoke("messages.getInlineBotResults",json!({"bot":bot,"peer":{"_":"inputPeerSelf"},"query":a::text(args,"query")?,"offset":string(args,"offset","")?})).await?;
        let query = id(&result["query_id"])?;
        let expires = chrono::Utc::now().timestamp() + result["cache_time"].as_i64().unwrap_or(0);
        let rows:Vec<_>=result["results"].as_array().into_iter().flatten().take(n as usize).enumerate().map(|(i,r)|json!({"index":i,"gif_id":format!("gif:{account}:{expires}:{query}:{}",r["id"].as_str().unwrap_or("")),"title":r["title"],"type":r["type"]})).collect();
        return Ok(
            json!({"records":rows,"next_offset":result["next_offset"],"expires_at":expires}),
        );
    }
    let raw = a::text(args, "gif_id")?;
    let parts: Vec<_> = raw.splitn(5, ':').collect();
    ensure!(
        parts.len() == 5 && parts[0] == "gif",
        "gif_id must be the opaque handle from get_gif_search"
    );
    ensure!(
        parts[1] == account,
        "GIF handle belongs to a different account"
    );
    let expiry = parts[2]
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("Malformed GIF handle"))?;
    ensure!(
        chrono::Utc::now().timestamp() < expiry,
        "GIF handle expired; search again"
    );
    let query = parts[3]
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("Malformed GIF query ID"))?;
    ensure!(!parts[4].is_empty(), "GIF result ID is empty");
    let mut p = json!({"peer":a::peer(api,args,"chat_id").await?,"query_id":query.to_string(),"id":parts[4],"random_id":a::random_id()});
    if let Some(t) = args.get("topic_id").filter(|v| !v.is_null()) {
        p["reply_to"] = json!({"_":"inputReplyToMessage","reply_to_msg_id":positive(t)?});
    }
    api.invoke("messages.sendInlineBotResult", p).await
}
async fn mini_app(api: &dyn Telegram, args: &Value) -> Result<Value> {
    ensure!(
        args.get("url").filter(|v| !v.is_null()).is_none()
            || args.get("short_name").filter(|v| !v.is_null()).is_none(),
        "Choose url or short_name, not both"
    );
    let bot = user(&a::peer(api, args, "bot").await?)?;
    let p = if let Some(chat) = args.get("chat_id").filter(|v| !v.is_null()) {
        api.resolve_peer(chat).await?
    } else {
        json!({"_":"inputPeerSelf"})
    };
    let mut params = json!({"peer":p,"bot":bot,"platform":"android"});
    copy_optional(args, &mut params, &["start_param"]);
    let method = if let Some(url) = args.get("url").filter(|v| !v.is_null()) {
        params["url"] = url.clone();
        "messages.requestWebView"
    } else if let Some(s) = args.get("short_name").filter(|v| !v.is_null()) {
        params.as_object_mut().unwrap().remove("bot");
        params["app"] = json!({"_":"inputBotAppShortName","bot_id":bot,"short_name":s});
        "messages.requestAppWebView"
    } else {
        "messages.requestMainWebView"
    };
    let mut result = api.invoke(method, params).await?;
    result["credential_warning"] = json!(
        "This URL contains Telegram Mini App authorization data. Treat the complete URL as a credential and share it only with the account owner's intended app."
    );
    Ok(result)
}
async fn translate(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let language = a::text(args, "to_language")?;
    nonempty(language, 16, "Language code")?;
    let text = args.get("text").filter(|v| !v.is_null());
    let messages = args.get("message_ids").filter(|v| !v.is_null());
    ensure!(
        text.is_some() != messages.is_some(),
        "Give either text or chat_id with message_ids"
    );
    let mut p = json!({"to_lang":language});
    if let Some(body) = text {
        let parts = if let Some(v) = body.as_array() {
            v.clone()
        } else {
            vec![body.clone()]
        };
        ensure!(
            !parts.is_empty() && parts.len() <= 100,
            "Expected 1..=100 text entries"
        );
        let mut values = Vec::new();
        for s in parts {
            let s = s
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Translation text must contain strings"))?;
            nonempty(s, 25000, "Translation text")?;
            values.push(txt(s));
        }
        p["text"] = json!(values);
    } else {
        p["peer"] = a::peer(api, args, "chat_id").await?;
        p["id"] = json!(ids(messages.unwrap(), 100)?);
    }
    api.invoke("messages.translateText", p).await
}
async fn stars(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let mid = positive(a::required(args, "message_id")?)?;
    let p = api.resolve_peer(&json!(777000)).await?;
    let msg = message(api, &p, mid).await?;
    let action = &msg["action"];
    ensure!(
        action["_"] == "messageActionGiftStars",
        "Message is not a Stars gift service receipt"
    );
    let reference = action["transaction_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Receipt has no transaction reference"))?;
    ensure!(
        !reference.is_empty()
            && reference.len() <= 512
            && reference.bytes().all(|b| (33..=126).contains(&b)),
        "Invalid transaction reference in service receipt"
    );
    let r=api.invoke("payments.getStarsTransactionsByID",json!({"peer":{"_":"inputPeerSelf"},"id":[{"_":"inputStarsTransaction","id":reference}]})).await?;
    let history = r["history"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Stars ledger record unavailable"))?;
    ensure!(
        history.len() == 1 && history[0]["id"] == reference,
        "Stars ledger record does not match the service receipt"
    );
    Ok(json!({"status":"observed","message_id":mid,"receipt":action,"transaction":history[0]}))
}
