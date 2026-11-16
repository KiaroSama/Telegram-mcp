use super::*;

pub(super) async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let generic = matches!(name, "promote_admin" | "edit_admin_rights");
    let mut mapped = a.clone();
    if name == "promote_admin" {
        let rights = a.get("rights").filter(|v| !v.is_null());
        let supplied = rights
            .map(|v| v.as_object().context("rights must be an object"))
            .transpose()?;
        if let Some(supplied) = supplied {
            ensure!(
                supplied.keys().all(|field| ADMIN.contains(&field.as_str())),
                "Unknown admin right"
            );
            ensure!(
                supplied.values().all(Value::is_boolean),
                "Rights must be true or false"
            );
        }
        for field in ADMIN {
            let default = [
                "change_info",
                "post_messages",
                "edit_messages",
                "delete_messages",
                "ban_users",
                "invite_users",
                "pin_messages",
                "manage_call",
                "manage_topics",
                "other",
            ]
            .contains(field);
            mapped[*field] = json!(
                supplied
                    .and_then(|r| r.get(*field))
                    .and_then(Value::as_bool)
                    .unwrap_or(default)
            );
        }
        mapped["rank"] = json!("Admin");
    }
    let a = &mapped;
    let mut demote = name == "demote_admin";
    let mut kind_name = if name.ends_with("community") {
        "community"
    } else if name.ends_with("channel") {
        "channel"
    } else {
        "group"
    };
    let key = if generic {
        if name == "promote_admin" {
            "group_id"
        } else {
            "chat_id"
        }
    } else if kind_name == "community" {
        "community"
    } else if kind_name == "channel" {
        "channel_id"
    } else {
        "group_id"
    };
    let mode = string(a, "mode", "replace")?;
    ensure!(
        ["replace", "patch"].contains(&mode.as_str()),
        "mode must be replace or patch"
    );
    let rank = a
        .get("rank")
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().context("rank must be a string"))
        .transpose()?;
    ensure!(
        rank.is_none_or(|r| r.chars().count() <= 16),
        "rank must have at most 16 characters"
    );
    let (p, info) = if kind_name == "community" {
        let (info, c) = super::super::communities::resolve(api, req(a, key)?).await?;
        (
            json!({"_":"inputPeerChannel","channel_id":c["channel_id"],"access_hash":c["access_hash"]}),
            info,
        )
    } else {
        let p = peer(api, a, key).await?;
        let info = chat_info(api, &p).await?;
        (p, info)
    };
    if demote && kind(&p) == "inputPeerChat" {
        let u = peer(api, a, "user_id").await?;
        let result = api
            .invoke(
                "messages.editChatAdmin",
                json!({"chat_id":entity_id(&p)?,"user_id":user(&u)?,"is_admin":false}),
            )
            .await?;
        let accepted = result.as_bool().context(
            "Telegram returned an unexpected admin response; the role change outcome is unverified",
        )?;
        return Ok(
            json!({"accepted":accepted,"admin_level":"basic_group","granular_rights_applied":false,"result":result}),
        );
    }
    if generic {
        for field in ADMIN {
            if let Some(value) = a.get(*field).filter(|value| !value.is_null()) {
                ensure!(value.is_boolean(), "Rights must be true or false");
            }
        }
        ensure!(
            is_channel(&p) || kind(&p) == "inputPeerChat",
            "Admin rights require a group or channel"
        );
        if kind(&p) == "inputPeerChat" {
            ensure!(
                name == "promote_admin",
                "Granular rights require a supergroup or channel; upgrade the basic group first"
            );
            let u = peer(api, a, "user_id").await?;
            let result = api
                .invoke(
                    "messages.editChatAdmin",
                    json!({"chat_id":entity_id(&p)?,"user_id":user(&u)?,"is_admin":true}),
                )
                .await?;
            let accepted=result.as_bool().context("Telegram returned an unexpected admin response; the role change outcome is unverified")?;
            return Ok(
                json!({"accepted":accepted,"admin_level":"basic_group","granular_rights_applied":false,"result":result}),
            );
        }
        kind_name = if info["broadcast"] == true {
            "channel"
        } else {
            "group"
        };
        if name == "edit_admin_rights" && mode == "replace" {
            demote = !ADMIN.iter().any(|field| a[*field] == true);
        }
    }
    let own = if generic {
        ADMIN
    } else {
        match kind_name {
            "community" => COMMUNITY,
            "channel" => CHANNEL,
            _ => GROUP,
        }
    };
    if !demote {
        ensure!(
            kind_name == "community" && kind(&info) == "community"
                || kind_name == "channel" && info["broadcast"] == true
                || kind_name == "group" && info["megagroup"] == true,
            "This chat has a different type; use the corresponding admin tool (basic groups must be upgraded first)"
        );
        for field in ADMIN {
            if let Some(v) = a.get(*field).filter(|v| !v.is_null()) {
                ensure!(v.is_boolean(), "Rights must be true or false");
                ensure!(
                    own.contains(field),
                    "{field} is not an admin right of a {kind_name}"
                );
                ensure!(
                    generic || *field != "manage_topics" || info["forum"] == true,
                    "manage_topics applies only to forum groups"
                );
            }
        }
    }
    let u = peer(api, a, "user_id").await?;
    let input_user = user(&u)?;
    let mut rights = json!({"_":"chatAdminRights"});
    let mut title = rank.unwrap_or("").to_owned();
    let exact = generic || name.starts_with("edit_");
    if !demote && mode == "patch" {
        let current = api
            .invoke(
                "channels.getParticipant",
                json!({"channel":channel(&p)?,"participant":u}),
            )
            .await?;
        let participant = &current["participant"];
        ensure!(
            matches!(
                kind(participant),
                "channelParticipantAdmin" | "channelParticipantCreator"
            ),
            "Telegram returned an unknown or non-admin participant; no rights were changed"
        );
        ensure!(
            participant["admin_rights"].is_object(),
            "This participant has no admin rights to patch"
        );
        let is_self = kind(&u) == "inputPeerSelf" || participant["is_self"] == true;
        if is_self {
            ensure!(
                kind(participant) == "channelParticipantCreator",
                "Only the creator can change their own admin settings"
            );
            ensure!(
                !ADMIN
                    .iter()
                    .any(|k| *k != "anonymous" && a.get(*k).is_some_and(|v| !v.is_null())),
                "The creator can change only their own anonymous flag and title"
            );
        }
        rights = participant["admin_rights"].clone();
        copy_present(a, &mut rights, ADMIN);
        if rank.is_none() {
            title = string(participant, "rank", "")?;
        }
        ensure!(
            ADMIN.iter().any(|k| *k != "other" && rights[*k] == true),
            "A patch cannot remove all admin rights; use demote_admin"
        );
        if rights == participant["admin_rights"] && title == string(participant, "rank", "")? {
            return Ok(json!({"unchanged":true}));
        }
    } else if !demote {
        for field in own {
            if *field == "manage_topics" && info["forum"] != true {
                continue;
            }
            rights[*field] = json!(boolean(a, field, !exact && *field != "anonymous")?);
        }
        ensure!(
            own.iter().any(|k| rights[*k] == true),
            "No right would be granted; name at least one true right or use demote_admin"
        );
    }
    let accepted = api
        .invoke(
            "channels.editAdmin",
            json!({"channel":channel(&p)?,"user_id":input_user,"admin_rights":rights,"rank":title}),
        )
        .await?;
    let observed = api
        .invoke(
            "channels.getParticipant",
            json!({"channel":channel(&p)?,"participant":u}),
        )
        .await;
    let declined = observed.as_ref().ok().map(|r| {
        ADMIN
            .iter()
            .filter(|k| rights[**k] == true && r["participant"]["admin_rights"][**k] != true)
            .copied()
            .collect::<Vec<_>>()
    });
    Ok(
        json!({"accepted":true,"verified":observed.is_ok(),"requested_rights":rights,"declined":declined,"result":accepted}),
    )
}
