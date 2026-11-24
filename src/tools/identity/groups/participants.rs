use super::*;

fn role(participant: &Value) -> &str {
    match kind(participant) {
        "channelParticipantCreator" | "chatParticipantCreator" => "creator",
        "channelParticipantAdmin" | "chatParticipantAdmin" => "admin",
        "channelParticipantBanned" if participant["banned_rights"]["view_messages"] == true => {
            "banned"
        }
        "channelParticipantBanned" if participant["left"] == true => "left",
        "channelParticipantBanned" => "restricted",
        "channelParticipantLeft" => "left",
        "channelParticipant" | "channelParticipantSelf" | "chatParticipant" => "member",
        _ => "unknown",
    }
}
fn rights(participant: &Value) -> Result<Value> {
    let mut output = json!({});
    for field in &crate::tl::Schema::global()
        .definition("chatAdminRights")?
        .fields
    {
        if field.kind != "#" {
            output[&field.name] = json!(participant["admin_rights"][&field.name] == true);
        }
    }
    Ok(output)
}
fn described(user: &Value, participant: &Value) -> Result<Value> {
    Ok(
        json!({"id":user["id"],"name":display_name(user),"username":user.get("username"),"role":role(participant),"rank":participant.get("rank"),"rights":rights(participant)?}),
    )
}
pub(super) async fn status(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let p = peer(api, a, "chat_id").await?;
    let u = peer(api, a, "user_id").await?;
    let c = channel(&p)?;
    let response = api
        .invoke(
            "channels.getParticipant",
            json!({"channel":c,"participant":u}),
        )
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) if error.to_string().contains("USER_NOT_PARTICIPANT") => {
            json!({"participant":{"_":"channelParticipantLeft"}})
        }
        Err(error) => return Err(error),
    };
    let participant = &response["participant"];
    let role = role(participant);
    let user_id = participant
        .get("user_id")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| entity_id(&u))?;
    Ok(
        json!({"results":[{"user_id":user_id,"role":role,"is_creator":role=="creator","is_admin":role=="creator"||role=="admin","rank":string(participant,"rank","")?,"rights":rights(participant)?}]}),
    )
}
pub(super) async fn list(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let page = int(a, "page", 1)?;
    ensure!(page > 0, "page must be positive");
    let size = limit(a, "page_size", 200, 200)?;
    let offset = (page - 1)
        .checked_mul(size)
        .context("page offset overflow")?;
    ensure!(
        offset < 100_000,
        "Paging past 100000 members is not supported"
    );
    let p = match peer(api, a, "chat_id").await {
        Ok(peer) => peer,
        Err(error) if name == "get_admins" => {
            match super::super::communities::resolve(api, req(a, "chat_id")?).await {
                Ok((_, channel)) => {
                    json!({"_":"inputPeerChannel","channel_id":channel["channel_id"],"access_hash":channel["access_hash"]})
                }
                Err(_) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    if !is_channel(&p) {
        ensure!(
            kind(&p) == "inputPeerChat",
            "Participants require a group or channel"
        );
        ensure!(
            name != "get_banned_users",
            "Basic groups do not retain a ban list; a supergroup is required"
        );
        let r = full(api, &p).await?;
        let participants = r["full_chat"]["participants"]["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut records = Vec::new();
        let selected = participants
            .iter()
            .filter(|p| name != "get_admins" || matches!(role(p), "creator" | "admin"));
        for participant in selected
            .skip(offset as usize)
            .take(if name == "get_admins" {
                100_000
            } else {
                size as usize
            })
        {
            let user = r
                .get("users")
                .and_then(Value::as_array)
                .and_then(|xs| xs.iter().find(|u| u["id"] == participant["user_id"]))
                .context("Participant page is missing a required user")?;
            records.push(described(user, participant)?);
        }
        return Ok(json!({"results":records,"untrusted_content":true}));
    }
    let filter = match name {
        "get_banned_users" => json!({"_":"channelParticipantsKicked","q":""}),
        "get_admins" => json!({"_":"channelParticipantsAdmins"}),
        _ => json!({"_":"channelParticipantsRecent"}),
    };
    let mut request =
        json!({"channel":channel(&p)?,"filter":filter,"offset":offset,"limit":size,"hash":0});
    if name != "get_admins" {
        return api.invoke("channels.getParticipants", request).await;
    }
    let mut all = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut complete = false;
    for _ in 0..500 {
        let response = api
            .invoke("channels.getParticipants", request.clone())
            .await?;
        let batch = response
            .get("participants")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = batch.len();
        let before = all.len();
        for participant in batch {
            if seen.insert(participant["user_id"].to_string()) {
                let user = response
                    .get("users")
                    .and_then(Value::as_array)
                    .and_then(|xs| xs.iter().find(|u| u["id"] == participant["user_id"]))
                    .context("Admin page is missing a required user")?;
                all.push(described(user, &participant)?);
            }
        }
        if count < size as usize {
            complete = true;
            break;
        }
        if all.len() == before {
            break;
        }
        request["offset"] = json!(int(&request, "offset", 0)? + size);
    }
    Ok(json!({"results":all,"complete":complete,"untrusted_content":true}))
}
