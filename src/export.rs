//! Export jobs retain an owned backend, and never borrow an MCP request task.

pub mod admin_log;
pub mod admin_log_text;
pub mod files;
pub mod html_admin;
pub mod icc;
pub mod jobs;
pub mod json_admin;
pub mod media;
pub mod render;
pub mod rich;
pub mod service;
pub mod settings;
pub mod thumbnail;
pub mod writer;

use anyhow::{Context, Result, bail};
use jobs::Cancelled;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;
use tokio::sync::watch;

const PAGE_SIZE: i64 = 100;
const MAX_RECORDS: usize = 1_000_000;

#[derive(Default)]
struct MediaPassStats {
    files: usize,
    refreshed: u64,
    failed_items: Vec<Value>,
    icc_converted: usize,
    icc_unchanged: usize,
    icc_fallback: usize,
}

impl MediaPassStats {
    fn absorb(&mut self, item: &Value, record_id: i64, role: &str) {
        self.refreshed += item["export_refreshed"].as_u64().unwrap_or(0);
        if item["export_file_failed"] == true {
            self.failed_items.push(json!({
                "id": record_id.to_string(),
                "role": role,
                "reason": item["export_file_skipped"].as_str().unwrap_or("download failed"),
            }));
        }
        match item["export_icc"]["outcome"].as_str() {
            Some("converted") => self.icc_converted += 1,
            Some("unchanged") => self.icc_unchanged += 1,
            Some("fallback") => self.icc_fallback += 1,
            _ => {}
        }
    }
}

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
        return Err(Cancelled.into());
    }
    let (method, params) = if let Some(id) = takeout {
        params["_"] = method.into();
        ("invokeWithTakeout", json!({"takeout_id":id,"query":params}))
    } else {
        (method, params)
    };
    tokio::select! {
        result = api.invoke(method, params) => result.context("Telegram export request failed"),
        _ = cancel.changed() => Err(Cancelled.into()),
    }
}

pub async fn run(
    api: &dyn crate::backend::Telegram,
    kind: &str,
    args: &Value,
    settings: &settings::Settings,
    cancel: watch::Receiver<bool>,
    job_id: &str,
) -> Result<Value> {
    run_inner(api, kind, args, settings, cancel, job_id, false).await
}

async fn run_inner(
    api: &dyn crate::backend::Telegram,
    kind: &str,
    args: &Value,
    settings: &settings::Settings,
    mut cancel: watch::Receiver<bool>,
    job_id: &str,
    reserved: bool,
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
        (&mut cancel, job_id, reserved),
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

struct HydrateEnv<'a, 'b> {
    api: &'a dyn crate::backend::Telegram,
    target: &'a Value,
    takeout: Option<&'a Value>,
    cancel: &'b mut watch::Receiver<bool>,
    settings: &'a settings::Settings,
    folder: &'a Path,
    emoji_cache: &'b mut BTreeMap<String, Option<String>>,
    stats: &'b mut MediaPassStats,
}

async fn hydrate_message_assets(
    env: &mut HydrateEnv<'_, '_>,
    message: &mut Value,
    record_id: i64,
    role: &str,
) -> Result<()> {
    if message["rich_message"]["part"] == true {
        let msg_id = numeric_id(&message["id"]).unwrap_or(record_id);
        let full = invoke(
            env.api,
            "messages.getRichMessage",
            json!({"peer": env.target, "id": msg_id}),
            env.takeout,
            env.cancel,
        )
        .await?;
        let rich = full["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| numeric_id(&m["id"]) == Some(msg_id))
            .and_then(|m| m.get("rich_message"))
            .context("Full rich message could not be loaded")?;
        if rich["part"] == true {
            bail!("Telegram returned a partial rich message during export");
        }
        message["rich_message"] = rich.clone();
    }
    if media::download_media(env.api, message, env.settings, env.folder).await? {
        env.stats.files += 1;
    }
    if media::download_service_photo(env.api, message, env.settings, env.folder).await? {
        env.stats.files += 1;
    }
    env.stats.files +=
        media::hydrate_custom_emoji(env.api, message, env.settings, env.folder, env.emoji_cache)
            .await?;
    if message.get("rich_message").is_some() {
        env.stats.files +=
            media::download_rich_media(env.api, message, env.settings, env.folder).await?;
    }
    env.stats.absorb(message, record_id, role);
    Ok(())
}

async fn hydrate_admin_event(
    env: &mut HydrateEnv<'_, '_>,
    event: &mut Value,
    event_id: i64,
) -> Result<()> {
    for key in ["message", "new_message", "prev_message"] {
        if event["action"][key].is_object() {
            let mut embedded = event["action"][key].clone();
            embedded["source_peer"] = env.target.clone();
            if let Some(mid) = numeric_id(&embedded["id"]) {
                embedded["source_message_id"] = mid.into();
            }
            embedded["source_event_id"] = event_id.into();
            hydrate_message_assets(env, &mut embedded, event_id, key).await?;
            event["action"][key] = embedded;
        }
    }
    if event["action"]["new_photo"]["_"].as_str() == Some("photo") {
        let mut carrier = json!({
            "id": event_id,
            "out": true,
            "source_peer": env.target,
            "source_event_id": event_id,
            "media": {
                "_": "messageMediaPhoto",
                "photo": event["action"]["new_photo"],
            }
        });
        if media::download_media(env.api, &mut carrier, env.settings, env.folder).await? {
            env.stats.files += 1;
            if let Some(rel) = carrier.get("export_file").cloned() {
                event["export_file"] = rel.clone();
                event["action"]["new_photo"]["export_file"] = rel;
            }
        }
        env.stats.absorb(&carrier, event_id, "new_photo");
    }
    Ok(())
}

async fn collect_and_write(
    api: &dyn crate::backend::Telegram,
    kind: &str,
    args: &Value,
    settings: &settings::Settings,
    peer: &Value,
    takeout: Option<&Value>,
    tracking: (&mut watch::Receiver<bool>, &str, bool),
) -> Result<Value> {
    let (cancel, job_id, reserved) = tracking;
    if *cancel.borrow() {
        return Err(Cancelled.into());
    }
    let state = crate::state::State::from_environment()?;
    let folder = state.resolve_file(&settings.path, crate::state::FileAccess::Write)?;
    if !reserved {
        if !folder.exists() {
            std::fs::create_dir(&folder).context(
                "Export directory could not be created; existing exports are never overwritten",
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
            }
        } else if files::directory_is_nonempty(&folder)? {
            bail!("Export directory could not be created; existing exports are never overwritten");
        }
    }
    let mut spool = tempfile::NamedTempFile::new_in(&folder)?;
    let mut offsets = Vec::new();
    let mut peers = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut emoji_cache = BTreeMap::new();
    let mut stats = MediaPassStats::default();
    let mut records = 0_usize;
    let mut targets = vec![(peer.clone(), false)];
    if kind == "chat" && peer["_"].as_str() == Some("inputPeerChannel") {
        let full = invoke(
            api,
            "channels.getFullChannel",
            json!({"channel":{"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]}}),
            takeout,
            cancel,
        )
        .await?;
        if let Some(id) =
            numeric_id(&full["full_chat"]["migrated_from_chat_id"]).filter(|id| *id > 0)
        {
            let legacy = json!({"_":"inputPeerChat","chat_id":id.to_string()});
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
            ranges.iter().rev().map(|r| Some(r.clone())).collect()
        }
    } else {
        vec![None]
    };

    for (target_index, (target, migrated)) in targets.iter().enumerate() {
        for range in &ranges {
            let mut offset = 0_i64;
            loop {
                if *cancel.borrow() {
                    return Err(Cancelled.into());
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
                            let pkind = if key == "users" {
                                "user"
                            } else if value["_"]
                                .as_str()
                                .is_some_and(|k| k.starts_with("channel"))
                            {
                                "channel"
                            } else {
                                "chat"
                            };
                            peers.insert(format!("{pkind}:{id}"), value.clone());
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
                    let mut env = HydrateEnv {
                        api,
                        target,
                        takeout,
                        cancel,
                        settings,
                        folder: &folder,
                        emoji_cache: &mut emoji_cache,
                        stats: &mut stats,
                    };
                    if kind == "recent_actions" {
                        let mut event = source_record.clone();
                        hydrate_admin_event(&mut env, &mut event, id).await?;
                        page.push(event);
                    } else {
                        let mut message = source_record.clone();
                        message["source_peer"] = target.clone();
                        message["source_message_id"] = id.into();
                        if *migrated {
                            message["id"] = (id - 1_000_000_000).into();
                            if message["reply_to"].is_object()
                                && message["reply_to"]
                                    .get("reply_to_peer_id")
                                    .is_none_or(Value::is_null)
                                && let Some(reply) =
                                    numeric_id(&message["reply_to"]["reply_to_msg_id"])
                            {
                                message["reply_to"]["reply_to_msg_id"] =
                                    (reply - 1_000_000_000).into();
                            }
                        }
                        hydrate_message_assets(&mut env, &mut message, id, "message").await?;
                        page.push(message);
                    }
                    records += 1;
                }
                if !page.is_empty() {
                    offsets.push(spool.as_file_mut().stream_position()?);
                    serde_json::to_writer(spool.as_file_mut(), &page)?;
                    spool.as_file_mut().write_all(b"\n")?;
                    spool.as_file_mut().flush()?;
                }
                jobs::progress(job_id, records, stats.files);
                if next_offset == i64::MAX || next_offset <= 0 {
                    break;
                }
                if offset != 0 && next_offset >= offset {
                    bail!("Export pagination did not advance; output is partial");
                }
                offset = next_offset;
                if past_start {
                    break;
                }
            }
        }
    }

    let base_title = writer::title_for_peer(peer, &peers);
    let written_messages = if kind == "recent_actions" {
        let title = format!("{base_title} - Recent actions");
        let channel_id = numeric_id(&peer["channel_id"]).unwrap_or(0);
        let mut log_chat = admin_log_text::LogChat::from_peers(channel_id, &peers, 0);
        log_chat.link_prefix = format!("https://{}/", crate::permalinks::link_domain());
        let mut all_events = Vec::with_capacity(records);
        for offset in offsets.into_iter().rev() {
            if *cancel.borrow() {
                return Err(Cancelled.into());
            }
            spool.as_file_mut().seek(SeekFrom::Start(offset))?;
            let mut line = String::new();
            BufReader::new(spool.as_file_mut()).read_line(&mut line)?;
            let mut page: Vec<Value> = serde_json::from_str(&line)?;
            page.reverse();
            all_events.extend(page);
        }
        all_events.sort_by_key(|ev| {
            (
                numeric_id(&ev["date"]).unwrap_or(0),
                numeric_id(&ev["id"]).unwrap_or(0),
            )
        });
        let chronological = admin_log::map_events_chronological(&all_events, &log_chat);
        let mut writer = writer::Writer::new_admin(&folder, settings, &title, peer)?;
        let mut emitted = 0_usize;
        for (raw_ev, entry) in all_events.iter().zip(chronological.iter()) {
            if *cancel.borrow() {
                return Err(Cancelled.into());
            }
            let (_, parts) = admin_log::map_event_parts(raw_ev, &log_chat);
            emitted += writer.admin_event(entry, &parts, &peers)?;
        }
        writer.finish()?;
        emitted
    } else {
        let mut writer = writer::Writer::new(&folder, settings, &base_title, peer)?;
        for offset in offsets.into_iter().rev() {
            if *cancel.borrow() {
                return Err(Cancelled.into());
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
        records
    };

    drop(spool);
    let partial = !stats.failed_items.is_empty();
    Ok(json!({
        "folder": folder,
        "format": settings.format,
        "messages": written_messages,
        "files": stats.files,
        "events": if kind == "recent_actions" { records } else { 0 },
        "complete": !partial,
        "partial": partial,
        "failed_files": stats.failed_items.len(),
        "failed_items": stats.failed_items,
        "refreshed_references": stats.refreshed,
        "icc_outcomes": {
            "converted": stats.icc_converted,
            "unchanged": stats.icc_unchanged,
            "fallback": stats.icc_fallback,
        },
        "format_compatibility": "Telegram Desktop export layout (messages.html and result.json)",
    }))
}

fn numeric_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
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
    let mut request = json!({
        "channel": {
            "_": "inputChannel",
            "channel_id": peer["channel_id"],
            "access_hash": peer["access_hash"],
        },
        "q": args["query"].as_str().unwrap_or(""),
        "max_id": offset.to_string(),
        "min_id": "0",
        "limit": PAGE_SIZE,
    });
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
