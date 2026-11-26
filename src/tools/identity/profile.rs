mod privacy;

use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub const NAMES: &[&str] = &[
    "get_me",
    "update_profile",
    "set_profile_photo",
    "delete_profile_photo",
    "get_full_user",
    "get_bot_info",
    "set_bot_commands",
    "get_user_photos",
    "get_user_status",
    "get_bot_commands",
    "get_privacy_settings",
    "set_privacy_settings",
    "set_bot_info",
    "invite_users_to_bot",
    "set_my_username",
    "list_authorizations",
    "terminate_authorization",
    "set_authorization_secret_chats",
    "set_authorization_calls",
    "set_secret_chats_only_device",
];

async fn self_user(api: &dyn Telegram) -> Result<Value> {
    let r = api
        .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
        .await?;
    r.as_array()
        .and_then(|xs| xs.first())
        .cloned()
        .context("Telegram returned no account identity")
}
async fn target_user(api: &dyn Telegram, a: &Value, key: &str) -> Result<Value> {
    if let Some(v) = a.get(key).filter(|v| !v.is_null()) {
        user(&api.resolve_peer(v).await?)
    } else {
        Ok(json!({"_":"inputUserSelf"}))
    }
}
async fn photos(api: &dyn Telegram, u: &Value, maximum: i64) -> Result<Value> {
    api.invoke(
        "photos.getUserPhotos",
        json!({"user_id":u,"offset":0,"max_id":0,"limit":maximum}),
    )
    .await
}
async fn previous_photos(api: &dyn Telegram, target: &Value) -> Result<Vec<Value>> {
    let mut photos = Vec::new();
    let mut offset = 0;
    for _ in 0..100 {
        let result = api
            .invoke(
                "photos.getUserPhotos",
                json!({"user_id":target,"offset":offset,"max_id":0,"limit":100}),
            )
            .await?;
        let page = result
            .get("photos")
            .and_then(Value::as_array)
            .context("Telegram returned no profile photo history; nothing was uploaded")?;
        photos.extend(
            page.iter()
                .filter(|photo| kind(photo) != "photoEmpty")
                .map(input_photo)
                .collect::<Result<Vec<_>>>()?,
        );
        offset += page.len();
        if page.len() < 100
            || result["count"]
                .as_u64()
                .is_some_and(|count| offset as u64 >= count)
        {
            return Ok(photos);
        }
    }
    anyhow::bail!("Profile photo history exceeds the bounded scan; nothing was uploaded or deleted")
}
pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "get_privacy_settings" || name == "set_privacy_settings" {
        return privacy::run(api, name, a).await;
    }
    if name == "list_authorizations"
        || name.contains("authorization")
        || name == "set_secret_chats_only_device"
    {
        return authorizations(api, name, a).await;
    }
    match name {
        "get_me" => {
            api.invoke("users.getFullUser", json!({"id":{"_":"inputUserSelf"}}))
                .await
        }
        "update_profile" => {
            let mut request = json!({});
            copy_present(a, &mut request, &["first_name", "last_name", "about"]);
            ensure!(
                !request.as_object().context("profile arguments")?.is_empty(),
                "Pass at least one profile field to update"
            );
            for key in ["first_name", "last_name", "about"] {
                if let Some(v) = request.get(key) {
                    ensure!(v.is_string(), "{key} must be a string");
                }
            }
            api.invoke("account.updateProfile", request).await
        }
        "set_my_username" => {
            let clean = username(text(a, "username")?, true)?;
            api.invoke("account.updateUsername", json!({"username":clean}))
                .await
        }
        "invite_users_to_bot" => {
            let selectors = a["user_ids"]
                .as_array()
                .context("user_ids must be an array")?;
            ensure!(
                (1..=100).contains(&selectors.len()),
                "Provide 1 to 100 user IDs"
            );
            let bot = user(&api.resolve_peer(&a["bot"]).await?)?;
            ensure!(bot["_"] == "inputUser", "Provide an explicit bot account");
            let mut users = Vec::with_capacity(selectors.len());
            for selector in selectors {
                let target = user(&api.resolve_peer(selector).await?)?;
                if !users.contains(&target) {
                    users.push(target);
                }
            }
            let requested_count = users.len();
            let ok = api
                .invoke(
                    "messages.inviteUsersToBot",
                    json!({"bot":bot,"users":users}),
                )
                .await?
                .as_bool()
                .context("Telegram returned no invitation result")?;
            Ok(json!({"ok":ok,"bot":bot["user_id"],"requested_count":requested_count}))
        }
        "set_bot_info" => {
            let mut p = json!({"bot":target_user(api,a,"bot").await?,"lang_code":string(a,"lang_code","")?});
            ensure!(
                ["name", "about", "description"]
                    .iter()
                    .any(|k| a.get(*k).is_some_and(|v| !v.is_null())),
                "Pass at least one bot information field"
            );
            copy_present(a, &mut p, &["name", "about", "description"]);
            api.invoke("bots.setBotInfo", p).await
        }
        "get_full_user" | "get_bot_info" => {
            let p = peer(
                api,
                a,
                if name == "get_full_user" {
                    "username"
                } else {
                    "bot_username"
                },
            )
            .await?;
            api.invoke("users.getFullUser", json!({"id":user(&p)?}))
                .await
        }
        "get_user_status" => {
            let p = peer(api, a, "user_id").await?;
            let r = api
                .invoke("users.getUsers", json!({"id":[user(&p)?]}))
                .await?;
            let u = r
                .as_array()
                .and_then(|xs| xs.first())
                .context("User not found")?;
            Ok(
                json!({"id":u["id"],"name":display_name(u),"status":u["status"],"untrusted_content":true}),
            )
        }
        "get_user_photos" => {
            let maximum = limit(a, "limit", 10, 100)?;
            let p = peer(api, a, "user_id").await?;
            photos(api, &user(&p)?, maximum).await
        }
        "set_bot_commands" | "get_bot_commands" => {
            let mut commands = Vec::new();
            if name == "set_bot_commands" {
                let entries = array(a, "commands")?;
                ensure!(entries.len() <= 100, "At most 100 commands are allowed");
                let mut seen = std::collections::HashSet::new();
                for c in entries {
                    let command = text(c, "command")?;
                    let description = text(c, "description")?;
                    ensure!(
                        !command.is_empty()
                            && command.len() <= 32
                            && command
                                .bytes()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                        "Commands must contain 1-32 lowercase ASCII letters, digits or underscores without a slash"
                    );
                    ensure!(
                        !description.is_empty() && description.chars().count() <= 256,
                        "Command descriptions must contain 1-256 characters"
                    );
                    ensure!(seen.insert(command), "Duplicate bot command");
                    commands.push(
                        json!({"_":"botCommand","command":command,"description":description}),
                    );
                }
            }
            ensure!(
                self_user(api).await?["bot"] == true,
                "Bot command menus can only be managed while signed in as a bot"
            );
            let mut p = json!({"scope":{"_":"botCommandScopeDefault"},"lang_code":""});
            if name == "set_bot_commands" {
                p["commands"] = commands.into();
            }
            api.invoke(
                if name == "set_bot_commands" {
                    "bots.setBotCommands"
                } else {
                    "bots.getBotCommands"
                },
                p,
            )
            .await
        }
        "set_profile_photo" => {
            let path = Path::new(text(a, "file_path")?);
            let replace = boolean(a, "replace", false)?;
            ensure!(
                !(replace && a.get("bot").is_some_and(|value| !value.is_null())),
                "Telegram cannot target a selected bot's historical photo deletion; set its current photo without replace"
            );
            let target = target_user(api, a, "bot").await?;
            let previous = if replace {
                previous_photos(api, &target).await?
            } else {
                Vec::new()
            };
            let uploaded = api.upload(path).await?;
            let mut request = json!({"file":uploaded});
            if a.get("bot").is_some_and(|v| !v.is_null()) {
                request["bot"] = target;
            }
            let result = api.invoke("photos.uploadProfilePhoto", request).await?;
            if previous.is_empty() {
                return Ok(result);
            }
            let mut deleted = Vec::new();
            for ids in previous.chunks(100) {
                match api.invoke("photos.deletePhotos", json!({"id":ids})).await {
                    Ok(reply) if reply.is_array() => {
                        deleted.extend(reply.as_array().into_iter().flatten().cloned())
                    }
                    other => {
                        let refused = matches!(other,Err(ref error) if error.chain().any(|cause|cause.to_string().starts_with("Telegram RPC ")));
                        return Ok(
                            json!({"photo_changed":true,"old_photos_deleted":if refused{json!(false)}else{Value::Null},"confirmed_deleted":deleted,"warning":"The new photo is set, but prior history cleanup was not confirmed; do not upload again","result":result}),
                        );
                    }
                }
            }
            let complete = previous.iter().all(|photo| {
                deleted.iter().any(|id| {
                    match (
                        int_value(id, "deleted photo id"),
                        int_value(&photo["id"], "old photo id"),
                    ) {
                        (Ok(deleted), Ok(old)) => deleted == old,
                        _ => false,
                    }
                })
            });
            Ok(
                json!({"photo_changed":true,"old_photos_deleted":deleted,"history_removed":complete,"result":result}),
            )
        }
        "delete_profile_photo" => {
            let wanted = a
                .get("photo_id")
                .filter(|v| !v.is_null())
                .map(|v| int_value(v, "photo_id"))
                .transpose()?;
            ensure!(
                wanted != Some(0),
                "photo_id must be a nonzero Telegram photo id"
            );
            ensure!(
                wanted.is_none() || a.get("bot").is_none_or(Value::is_null),
                "Telegram cannot target deletion of a selected bot's historical photo; omit photo_id to clear its current photo"
            );
            let target = target_user(api, a, "bot").await?;
            if wanted.is_none() && a.get("bot").is_some_and(|v| !v.is_null()) {
                return api
                    .invoke(
                        "photos.updateProfilePhoto",
                        json!({"bot":target,"id":{"_":"inputPhotoEmpty"}}),
                    )
                    .await;
            }
            // Locate the exact photo rather than deleting the current one on a failed match.
            let mut offset = 0;
            for _ in 0..1000 {
                let r=api.invoke("photos.getUserPhotos",json!({"user_id":target,"offset":offset,"max_id":0,"limit":if wanted.is_none(){1}else{100}})).await?;
                let list = r
                    .get("photos")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let matching = list
                    .iter()
                    .find(|p| wanted.is_none() || int_value(&p["id"], "photo id").ok() == wanted);
                if let Some(photo) = matching {
                    return api
                        .invoke("photos.deletePhotos", json!({"id":[input_photo(photo)?]}))
                        .await;
                }
                if list.len() < 100 {
                    break;
                }
                offset += list.len();
            }
            anyhow::bail!("No matching profile photo exists; no photo was deleted")
        }
        _ => unreachable!(),
    }
}

fn device(session: &Value) -> Value {
    let api = session["api_id"].as_i64().filter(|n| *n > 0);
    let app = session["app_name"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let support = match api {
        Some(2040 | 2496) => "not_supported",
        Some(_) => "supported",
        None if matches!(
            app.as_str(),
            "telegram desktop" | "telegram web" | "telegram webk" | "telegram weba"
        ) =>
        {
            "not_supported"
        }
        None => "unknown",
    };
    json!({"hash":session["hash"],"api_id":api,"device_model":session["device_model"],"app_name":session["app_name"],"platform":session["platform"],"current":session["current"]==true,"accept_calls":session["call_requests_disabled"]!=true,"accept_secret_chats":session["encrypted_requests_disabled"]!=true,"secret_chat_support":support})
}
async fn authorizations(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let hash = if name == "list_authorizations" {
        None
    } else {
        Some(int_value(req(a, "hash")?, "hash")?)
    };
    let switch = match name {
        "set_authorization_calls" => Some((
            "call_requests_disabled",
            boolean_required(a, "accept_calls")?,
        )),
        "set_authorization_secret_chats" => Some((
            "encrypted_requests_disabled",
            boolean_required(a, "accept_secret_chats")?,
        )),
        _ => None,
    };
    let r = api.invoke("account.getAuthorizations", json!({})).await?;
    let sessions = r["authorizations"]
        .as_array()
        .context("Telegram returned no authorization list")?;
    let Some(hash) = hash else {
        let mut r = r.clone();
        r["authorizations"] = json!(
            sessions
                .iter()
                .map(|s| {
                    let mut row = s.clone();
                    if let (Some(m), Some(fields)) = (row.as_object_mut(), device(s).as_object()) {
                        m.extend(fields.clone());
                    }
                    row
                })
                .collect::<Vec<_>>()
        );
        return Ok(r);
    };
    let target = sessions
        .iter()
        .find(|s| int_value(&s["hash"], "hash").ok() == Some(hash))
        .context("No authorization matches this hash; nothing changed")?;
    let summary = device(target);
    if name == "terminate_authorization" {
        ensure!(
            target["current"] != true || boolean(a, "include_current", false)?,
            "Refusing to terminate the current authorization without include_current=true"
        );
        return api
            .invoke(
                "account.resetAuthorization",
                json!({"hash":hash.to_string()}),
            )
            .await;
    }
    if let Some((key, allow)) = switch {
        if key == "encrypted_requests_disabled" && allow {
            ensure!(
                summary["secret_chat_support"] == "supported",
                "Secret-chat support is unavailable or unknown for this device"
            );
        }
        return api
            .invoke(
                "account.changeAuthorizationSettings",
                json!({"hash":hash.to_string(),key:!allow}),
            )
            .await;
    }
    ensure!(
        summary["secret_chat_support"] == "supported",
        "Secret-chat support is unavailable or unknown for this device"
    );
    let mut results = vec![];
    for (session, keep) in std::iter::once((target, true)).chain(
        sessions
            .iter()
            .filter(|s| int_value(&s["hash"], "hash").ok() != Some(hash))
            .map(|s| (s, false)),
    ) {
        let h = int_value(&session["hash"], "hash")?;
        let result = api
            .invoke(
                "account.changeAuthorizationSettings",
                json!({"hash":h.to_string(),"encrypted_requests_disabled":!keep}),
            )
            .await;
        let accepted = result.is_ok_and(|v| v == true);
        results.push(json!({"hash":h.to_string(),"accept_secret_chats":keep,"accepted":accepted}));
        if keep && !accepted {
            break;
        }
    }
    Ok(
        json!({"device":summary,"complete":results.iter().all(|r|r["accepted"]==true),"results":results}),
    )
}
fn boolean_required(a: &Value, key: &str) -> Result<bool> {
    req(a, key)?
        .as_bool()
        .with_context(|| format!("{key} must be boolean"))
}
