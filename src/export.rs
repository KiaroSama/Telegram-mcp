//! Export jobs retain an owned backend, and never borrow an MCP request task.

pub mod jobs;
pub mod media;
pub mod render;
pub mod rich;
pub mod settings;
mod writer;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use tokio::sync::watch;

const PAGE_SIZE: i64 = 100;
const MAX_RECORDS: usize = 1_000_000;

pub async fn call(
    api: &dyn crate::backend::Telegram,
    name: &str,
    args: &Value,
) -> Option<Result<Value>> {
    Some(match name {
        "export_status" => jobs::status(args),
        "cancel_export" => jobs::cancel(args),
        "export_chat_history" | "export_recent_actions" => (|| {
            let settings = settings::Settings::parse(args)?;
            crate::args::required(args, "chat_id")?;
            let owned = api
                .clone_handle()
                .context("Backend cannot create an owned export job")?;
            jobs::start(
                owned,
                if name == "export_recent_actions" {
                    "recent_actions"
                } else {
                    "chat"
                },
                args.clone(),
                settings,
            )
        })(),
        _ => return None,
    })
}

async fn invoke(
    api: &dyn crate::backend::Telegram,
    method: &str,
    mut params: Value,
    takeout: Option<&Value>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value> {
    if *cancel.borrow() {
        bail!("Export cancelled");
    }
    let (method, params) = if let Some(id) = takeout {
        params["_"] = method.into();
        ("invokeWithTakeout", json!({"takeout_id":id,"query":params}))
    } else {
        (method, params)
    };
    tokio::select! {
        result = api.invoke(method, params) => result.context("Telegram export request failed"),
        _ = cancel.changed() => bail!("Export cancelled"),
    }
}

pub async fn run(
    api: &dyn crate::backend::Telegram,
    kind: &str,
    args: &Value,
    settings: &settings::Settings,
    mut cancel: watch::Receiver<bool>,
    job_id: &str,
) -> Result<Value> {
    let peer = api
        .resolve_peer(crate::args::required(args, "chat_id")?)
        .await?;
    let has_media = settings.media.into_iter().any(|value| value);
    let takeout_result = if kind == "chat" {
        let mut request = json!({"contacts":false,"message_users":true,"message_chats":true,"message_megagroups":true,"message_channels":true,"files":has_media});
        if has_media {
            request["file_max_size"] = settings.max_bytes.into();
        }
        invoke(
            api,
            "account.initTakeoutSession",
            request,
            None,
            &mut cancel,
        )
        .await
    } else {
        Err(anyhow::anyhow!("Admin log uses ordinary reads"))
    };
    let takeout = takeout_result
        .as_ref()
        .ok()
        .and_then(|value| value.get("id"))
        .cloned();
    let result = collect_and_write(
        api,
        kind,
        args,
        settings,
        &peer,
        takeout.as_ref(),
        (&mut cancel, job_id),
    )
    .await;
    if let Some(id) = &takeout {
        let finish = json!({"takeout_id":id,"query":{"_":"account.finishTakeoutSession","success":result.is_ok()}});
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            api.invoke("invokeWithTakeout", finish),
        )
        .await;
    }
    let mut result = result?;
    result["takeout"] = takeout.is_some().into();
    if kind == "chat" && takeout.is_none() {
        result["takeout_refused"] = "Telegram refused takeout; ordinary reads were used".into();
    }
    Ok(result)
}

async fn collect_and_write(
    api: &dyn crate::backend::Telegram,
    kind: &str,
    args: &Value,
    settings: &settings::Settings,
    peer: &Value,
    takeout: Option<&Value>,
    tracking: (&mut watch::Receiver<bool>, &str),
) -> Result<Value> {
    let (cancel, job_id) = tracking;
    let state = crate::state::State::from_environment()?;
    let folder = state.resolve_file(&settings.path, crate::state::FileAccess::Write)?;
    std::fs::create_dir(&folder)
        .context("Export directory could not be created; existing exports are never overwritten")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut spool = tempfile::NamedTempFile::new_in(&folder)?;
    let mut offsets = Vec::new();
    let mut peers = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut emoji_cache = BTreeMap::new();
    let (mut files, mut records) = (0_usize, 0_usize);
    let mut targets = vec![(peer.clone(), false)];
    if kind == "chat" && peer["_"].as_str() == Some("inputPeerChannel") {
        let full=invoke(api,"channels.getFullChannel",json!({"channel":{"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]}}),takeout,cancel).await?;
        if let Some(id) =
            numeric_id(&full["full_chat"]["migrated_from_chat_id"]).filter(|id| *id > 0)
        {
            let legacy = json!({"_":"inputPeerChat","chat_id":id.to_string()});
            // Migration links identify a second chat, not an extension of the
            // requested supergroup's authorization. Check its explicit peer so
            // neither an allowed supergroup nor a topic-only entry widens it.
            if crate::server::access::AccessPolicy::from_environment()?
                .check("export_chat_history", &json!({"chat_id":legacy}), api)
                .await
                .is_ok()
            {
                targets.push((legacy, true));
            }
        }
    }
    let ranges = if kind == "chat" && takeout.is_some() {
        let value = invoke(api, "messages.getSplitRanges", json!({}), takeout, cancel).await?;
        let ranges = value
            .as_array()
            .context("Export split ranges are not a list")?;
        if ranges.len() > 4096 {
            bail!("Telegram returned more than 4096 history split ranges");
        }
        if ranges.is_empty() {
            vec![None]
        } else {
            ranges
                .iter()
                .rev()
                .map(|range| Some(range.clone()))
                .collect()
        }
    } else {
        vec![None]
    };
    for (target_index, (target, migrated)) in targets.iter().enumerate() {
        for range in &ranges {
            let mut offset = 0_i64;
            loop {
                if *cancel.borrow() {
                    bail!("Export cancelled");
                }
                let response = if kind == "recent_actions" {
                    invoke(
                        api,
                        "channels.getAdminLog",
                        admin_request(api, args, target, offset).await?,
                        takeout,
                        cancel,
                    )
                    .await?
                } else {
                    // TDesktop 08c62ec skips history outside the requested
                    // date window. We walk backwards, so jump from the upper
                    // bound on page one, then retain exact ID pagination.
                    let offset_date = if offset == 0 { settings.until } else { 0 };
                    let mut query = json!({"peer":target,"offset_id":offset,"offset_date":offset_date,"add_offset":0,"limit":PAGE_SIZE,"max_id":0,"min_id":0,"hash":"0"});
                    if let Some(range) = range {
                        query["_"] = "messages.getHistory".into();
                        invoke(
                            api,
                            "invokeWithMessagesRange",
                            json!({"range":range,"query":query}),
                            takeout,
                            cancel,
                        )
                        .await?
                    } else {
                        invoke(api, "messages.getHistory", query, takeout, cancel).await?
                    }
                };
                for key in ["users", "chats"] {
                    for value in response[key].as_array().into_iter().flatten() {
                        if let Some(id) = numeric_id(&value["id"]) {
                            let kind = if key == "users" {
                                "user"
                            } else if value["_"]
                                .as_str()
                                .is_some_and(|kind| kind.starts_with("channel"))
                            {
                                "channel"
                            } else {
                                "chat"
                            };
                            peers.insert(format!("{kind}:{id}"), value.clone());
                        }
                    }
                }
                let source = response[if kind == "recent_actions" {
                    "events"
                } else {
                    "messages"
                }]
                .as_array()
                .context("Export response does not contain a record list")?;
                if source.is_empty() {
                    break;
                }
                let mut next_offset = i64::MAX;
                let mut page = Vec::with_capacity(source.len());
                let mut past_start = true;
                for source_record in source {
                    let Some(id) = numeric_id(&source_record["id"]) else {
                        bail!("Export response record has no valid identifier");
                    };
                    next_offset = next_offset.min(id);
                    let date = numeric_id(&source_record["date"]).unwrap_or(0);
                    if date >= settings.from {
                        past_start = false;
                    }
                    if !settings.includes_date(date) || !seen.insert((target_index, id)) {
                        continue;
                    }
                    if records >= MAX_RECORDS {
                        bail!(
                            "Export reached the one-million-record bound; output is partial. Export smaller date ranges"
                        );
                    }
                    let mut message = if kind == "recent_actions" {
                        event_message(source_record)
                    } else {
                        source_record.clone()
                    };
                    if message["rich_message"]["part"] == true {
                        let full = invoke(
                            api,
                            "messages.getRichMessage",
                            json!({"peer":target,"id":id}),
                            takeout,
                            cancel,
                        )
                        .await?;
                        let rich = full["messages"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|m| numeric_id(&m["id"]) == Some(id))
                            .and_then(|m| m.get("rich_message"))
                            .context("Full rich message could not be loaded")?;
                        if rich["part"] == true {
                            bail!("Telegram returned a partial rich message during export");
                        }
                        message["rich_message"] = rich.clone();
                    }
                    message["source_peer"] = target.clone();
                    message["source_message_id"] = id.into();
                    if *migrated {
                        message["id"] = (id - 1_000_000_000).into();
                        if message["reply_to"].is_object()
                            && message["reply_to"].get("reply_to_peer_id").is_none()
                            && let Some(reply) = numeric_id(&message["reply_to"]["reply_to_msg_id"])
                        {
                            message["reply_to"]["reply_to_msg_id"] = (reply - 1_000_000_000).into();
                        }
                    }
                    if media::download_media(api, &mut message, settings, &folder).await? {
                        files += 1;
                    }
                    files += media::hydrate_custom_emoji(
                        api,
                        &mut message,
                        settings,
                        &folder,
                        &mut emoji_cache,
                    )
                    .await?;
                    if message.get("rich_message").is_some() {
                        files += media::download_rich_media(api, &mut message, settings, &folder)
                            .await?;
                    }
                    page.push(message);
                    records += 1;
                }
                if !page.is_empty() {
                    offsets.push(spool.as_file_mut().stream_position()?);
                    serde_json::to_writer(spool.as_file_mut(), &page)?;
                    spool.as_file_mut().write_all(b"\n")?;
                    spool.as_file_mut().flush()?;
                }
                jobs::progress(job_id, records, files);
                if next_offset == i64::MAX || next_offset <= 0 {
                    break;
                }
                if offset != 0 && next_offset >= offset {
                    bail!("Export pagination did not advance; output is partial");
                }
                offset = next_offset;
                if source.len() < PAGE_SIZE as usize || past_start {
                    break;
                }
            }
        }
    }
    let title = title_for_peer(peer, &peers);
    let mut writer = writer::Writer::new(&folder, settings, &title, peer)?;
    for offset in offsets.into_iter().rev() {
        if *cancel.borrow() {
            bail!("Export cancelled");
        }
        spool.as_file_mut().seek(SeekFrom::Start(offset))?;
        let mut line = String::new();
        BufReader::new(spool.as_file_mut()).read_line(&mut line)?;
        let mut page: Vec<Value> = serde_json::from_str(&line)?;
        page.reverse();
        for message in page {
            writer.message(&message, &peers)?;
        }
    }
    writer.finish()?;
    Ok(
        json!({"folder":folder,"format":settings.format,"messages":records,"files":files,
        "events":if kind=="recent_actions" {records}else{0},"complete":true,
        "format_compatibility":"Rust structural export; not byte-identical to Telegram Desktop's Qt exporter",
        "data_preservation":"Original Telegram TL fields are retained in result.json telegram_tl records"}),
    )
}

fn numeric_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn title_for_peer(peer: &Value, peers: &BTreeMap<String, Value>) -> String {
    let id = peer
        .get("user_id")
        .or_else(|| peer.get("channel_id"))
        .or_else(|| peer.get("chat_id"));
    let kind = match peer["_"].as_str() {
        Some("inputPeerUser") => "user",
        Some("inputPeerChannel") => "channel",
        _ => "chat",
    };
    id.and_then(numeric_id)
        .and_then(|id| peers.get(&format!("{kind}:{id}")))
        .map(|value| {
            value["title"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| render::person_name(value))
        })
        .unwrap_or_else(|| "Telegram chat export".into())
}

fn event_message(event: &Value) -> Value {
    let action = &event["action"];
    let mut message = action
        .get("message")
        .or_else(|| action.get("new_message"))
        .cloned()
        .unwrap_or_else(|| json!({"_":"messageService","message":""}));
    message["id"] = event["id"].clone();
    message["date"] = event["date"].clone();
    message["from_id"] = json!({"_":"peerUser","user_id":event["user_id"]});
    message["action"] = action.clone();
    message["admin_log_event"] = event.clone();
    message
}

async fn admin_request(
    api: &dyn crate::backend::Telegram,
    args: &Value,
    peer: &Value,
    offset: i64,
) -> Result<Value> {
    if peer["_"].as_str() != Some("inputPeerChannel") {
        bail!("Recent actions exist only in supergroups and channels");
    }
    let mut request = json!({"channel":{"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]},
        "q":args["query"].as_str().unwrap_or(""),"max_id":offset.to_string(),"min_id":"0","limit":PAGE_SIZE});
    if let Some(values) = args["event_types"].as_array().filter(|v| !v.is_empty()) {
        let mut filter = json!({"_":"channelAdminLogEventsFilter"});
        for name in values {
            let name = name
                .as_str()
                .context("event_types entries must be strings")?;
            let flags: &[&str] = match name {
                "admin_rights" => &["promote", "demote"],
                "tag_changes" => &["edit_rank"],
                "new_restrictions" => &["ban", "unban", "kick", "unkick"],
                "new_members" => &["join", "invite"],
                "members_leaving" => &["leave"],
                "group_info" => &["info", "settings"],
                "invite_links" => &["invites"],
                "video_chats" => &["group_call"],
                "subscription_renewals" => &["sub_extend"],
                "topics" => &["forums"],
                "deleted_messages" => &["delete"],
                "edited_messages" => &["edit"],
                "pinned_messages" => &["pinned"],
                _ => bail!("Unknown recent-action event type: {name}"),
            };
            for flag in flags {
                filter[*flag] = true.into();
            }
        }
        request["events_filter"] = filter;
    }
    if let Some(admins) = args["admins"].as_array().filter(|a| !a.is_empty()) {
        let mut users = Vec::with_capacity(admins.len());
        for admin in admins {
            let resolved = api.resolve_peer(admin).await?;
            let user = match resolved["_"].as_str() {
                Some("inputPeerUser") => {
                    json!({"_":"inputUser","user_id":resolved["user_id"],"access_hash":resolved["access_hash"]})
                }
                Some("inputPeerSelf") => json!({"_":"inputUserSelf"}),
                _ => bail!("Recent-action admins must resolve to users"),
            };
            users.push(user);
        }
        request["admins"] = users.into();
    }
    Ok(request)
}
