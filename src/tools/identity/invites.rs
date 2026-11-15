use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub const NAMES: &[&str] = &[
    "get_invite_link",
    "export_chat_invite",
    "import_chat_invite",
    "join_chat_by_link",
    "create_invite_link",
    "edit_invite_link",
    "revoke_invite_link",
    "list_invite_links",
    "list_join_requests",
    "approve_join_request",
    "list_invite_link_admins",
    "list_link_joins",
    "create_paid_invite_link",
    "get_invite_link_qr",
    "delete_revoked_invite_links",
];

fn invite_hash(link: &str) -> Result<String> {
    let trimmed = link.trim();
    let hash = if let Some(h) = trimmed.strip_prefix("tg://join?invite=") {
        h
    } else if let Some((_, h)) = trimmed.split_once("/joinchat/") {
        h
    } else if let Some((_, h)) = trimmed.split_once("/+") {
        h
    } else {
        trimmed
    };
    ensure!(
        !hash.is_empty()
            && hash
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Not a Telegram private invite link or hash"
    );
    Ok(hash.to_owned())
}
async fn admin(api: &dyn Telegram, a: &Value) -> Result<Value> {
    if let Some(admin) = a.get("admin").filter(|v| !v.is_null()) {
        user(&api.resolve_peer(admin).await?)
    } else {
        Ok(json!({"_":"inputUserSelf"}))
    }
}
fn expiry(a: &Value) -> Result<Option<i64>> {
    let Some(value) = a.get("expire_seconds").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let n = int_value(value, "expire_seconds")?;
    ensure!(n >= 0, "expire_seconds must be zero or greater");
    if n == 0 {
        return Ok(Some(0));
    }
    let stamp = now()?.checked_add(n).context("Expiry is too large")?;
    ensure!(
        stamp <= i32::MAX as i64,
        "Expiry exceeds Telegram's supported timestamp"
    );
    Ok(Some(stamp))
}
pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "import_chat_invite" || name == "join_chat_by_link" {
        let raw = text(
            a,
            if name == "import_chat_invite" {
                "hash"
            } else {
                "link"
            },
        )?;
        if name == "join_chat_by_link"
            && !raw.contains("/+")
            && !raw.contains("/joinchat/")
            && !raw.starts_with("tg://join")
        {
            let username = raw
                .trim()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_start_matches('@');
            ensure!(!username.is_empty(), "link must identify a Telegram chat");
            let p = api.resolve_peer(&json!(username)).await?;
            return api
                .invoke("channels.joinChannel", json!({"channel":channel(&p)?}))
                .await;
        }
        return api
            .invoke(
                "messages.importChatInvite",
                json!({"hash":invite_hash(raw)?}),
            )
            .await;
    }
    if name == "get_invite_link_qr" {
        let link = text(a, "link")?;
        ensure!(!link.trim().is_empty(), "link must not be empty");
        let code = qrcode::QrCode::new(link.as_bytes())?;
        let modules = code.width();
        let scale = 8;
        let border = 4;
        let edge = ((modules + border * 2) * scale) as u32;
        let mut image = image::GrayImage::from_pixel(edge, edge, image::Luma([255]));
        for y in 0..modules {
            for x in 0..modules {
                if code[(x, y)] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            image.put_pixel(
                                ((x + border) * scale + dx) as u32,
                                ((y + border) * scale + dy) as u32,
                                image::Luma([0]),
                            );
                        }
                    }
                }
            }
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image).write_to(&mut bytes, image::ImageFormat::Png)?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
        return Ok(
            json!({"content":[{"type":"image","data":data,"mimeType":"image/png"}],"metadata":{"link":link}}),
        );
    }
    let expiry = expiry(a)?;
    let usage = a
        .get("usage_limit")
        .filter(|v| !v.is_null())
        .map(|v| int_value(v, "usage_limit"))
        .transpose()?;
    ensure!(
        usage.is_none_or(|n| n >= 0),
        "usage_limit must be zero or greater"
    );
    if a.get("requires_approval") == Some(&Value::Bool(true)) {
        ensure!(
            usage.is_none_or(|n| n == 0),
            "An approval link cannot also have a usage limit"
        );
    }
    if name == "create_paid_invite_link" {
        ensure!(
            (1..=10000).contains(&int(a, "monthly_fee_stars", 0)?),
            "monthly_fee_stars must be 1 to 10000"
        );
    }
    let p = peer(api, a, "chat_id").await?;
    match name{
        "get_invite_link"=>{let r=full(api,&p).await?;Ok(json!({"invite":r["full_chat"]["exported_invite"]}))}
        "export_chat_invite"=>{
            let old=api.invoke("messages.getExportedChatInvites",json!({"peer":p,"admin_id":{"_":"inputUserSelf"},"limit":100})).await?;
            if let Some(invite)=old.get("invites").and_then(Value::as_array).and_then(|xs|xs.iter().find(|v|v["permanent"]==true&&v["revoked"]!=true)){
                let revoked = api.invoke("messages.editExportedChatInvite",json!({"peer":p,"link":req(invite,"link")?,"revoked":true})).await?;
                if kind(&revoked) == "messages.exportedChatInviteReplaced" {
                    let replacement = req(&revoked, "new_invite")?;
                    ensure!(kind(replacement) == "chatInviteExported" && replacement["revoked"] != true,
                        "Telegram replaced the primary invite but returned no usable replacement; inspect invite links before retrying");
                    return Ok(replacement.clone());
                }
            }
            api.invoke("messages.exportChatInvite",json!({"peer":p})).await
        }
        "create_invite_link"|"edit_invite_link"|"create_paid_invite_link"=>{
            let mut request=json!({"peer":p});copy_present(a,&mut request,&["title"]);
            if let Some(expiry)=expiry&& (name=="edit_invite_link"||expiry!=0){request["expire_date"]=expiry.into();}
            if let Some(usage)=usage&& (name=="edit_invite_link"||usage!=0){request["usage_limit"]=usage.into();}
            if let Some(v)=a.get("requires_approval").filter(|v|!v.is_null()){request["request_needed"]=json!(v.as_bool().context("requires_approval must be boolean")?);}
            if name=="edit_invite_link"{request["link"]=text(a,"link")?.into();}
            if name=="create_paid_invite_link"{
                ensure!(is_channel(&p),"Paid invite links require a private broadcast channel");let info=chat_info(api,&p).await?;
                ensure!(info["broadcast"]==true&&info["username"].as_str().is_none_or(str::is_empty),"Paid invite links require a private broadcast channel");
                request["subscription_pricing"]=json!({"_":"starsSubscriptionPricing","period":2592000,"amount":int(a,"monthly_fee_stars",0)?.to_string()});
            }
            api.invoke(if name=="edit_invite_link"{"messages.editExportedChatInvite"}else{"messages.exportChatInvite"},request).await
        }
        "revoke_invite_link"=>{
            let link=text(a,"link")?;
            let revoked=api.invoke("messages.editExportedChatInvite",json!({"peer":p,"link":link,"revoked":true})).await?;
            if boolean(a,"delete",false)?{
                match api.invoke("messages.deleteExportedChatInvite",json!({"peer":p,"link":link})).await{Ok(r)=>Ok(json!({"revoked":true,"deleted":r==true})),Err(_)=>Ok(json!({"revoked":true,"deleted":false,"warning":"Link was revoked but its record could not be deleted"}))}
            }else{Ok(revoked)}
        }
        "list_invite_links"=>api.invoke("messages.getExportedChatInvites",json!({"peer":p,"admin_id":admin(api,a).await?,"limit":limit(a,"limit",20,100)?,"revoked":boolean(a,"revoked",false)?})).await,
        "list_join_requests"|"list_link_joins"=>{
            let mut request=json!({"peer":p,"offset_date":0,"offset_user":{"_":"inputUserSelf"},"limit":limit(a,"limit",if name=="list_join_requests"{20}else{50},100)?});
            copy_present(a,&mut request,&["link"]);
            if name=="list_join_requests"{request["requested"]=true.into();}else{req(a,"link")?;request["subscription_expired"]=json!(boolean(a,"expired_subscriptions",false)?);}
            api.invoke("messages.getChatInviteImporters",request).await
        }
        "approve_join_request"=>{let u=peer(api,a,"user_id").await?;api.invoke("messages.hideChatJoinRequest",json!({"peer":p,"user_id":user(&u)?,"approved":boolean(a,"approved",true)?})).await}
        "list_invite_link_admins"=>api.invoke("messages.getAdminsWithInvites",json!({"peer":p})).await,
        "delete_revoked_invite_links"=>api.invoke("messages.deleteRevokedExportedChatInvites",json!({"peer":p,"admin_id":admin(api,a).await?})).await,
        _=>unreachable!(),
    }
}
