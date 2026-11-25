//! Local configuration, event waits and owner-controlled safety settings.
mod commands;
pub mod proxies;
mod unread;
pub mod upstream_events;
pub use unread::export_unread_with_state;

use crate::backend::Telegram;
use crate::state::{State, normalize_key};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if let Some(result) = upstream_events::call(api, name, args).await {
        return Some(result);
    }
    if let Some(result) = proxies::call(api, name, args).await {
        return Some(result);
    }
    if name == "list_chat_commands" {
        return Some(commands::list_chat_commands(api, args).await);
    }
    if !NAMES.contains(&name) {
        return None;
    }
    Some(dispatch(api, name, args).await)
}

pub const NAMES: &[&str] = &[
    "list_accounts",
    "check_account_session",
    "get_file_roots_status",
    "set_contact_alias",
    "list_contact_aliases",
    "delete_contact_alias",
    "set_ghost_mode",
    "get_ghost_mode",
    "safeguard_status",
    "revoke_always_approval",
    "wait_for_new_message",
    "wait_for_settled_message",
    "enable_incoming_feed",
    "disable_incoming_feed",
    "incoming_feed_status",
];

pub const TOOL_NAMES: &[&str] = &[
    "list_accounts",
    "check_account_session",
    "get_file_roots_status",
    "set_contact_alias",
    "list_contact_aliases",
    "delete_contact_alias",
    "set_ghost_mode",
    "get_ghost_mode",
    "safeguard_status",
    "revoke_always_approval",
    "wait_for_new_message",
    "wait_for_settled_message",
    "enable_incoming_feed",
    "disable_incoming_feed",
    "incoming_feed_status",
    "list_chat_commands",
    "add_proxies",
    "list_proxies",
    "remove_proxies",
    "test_proxies",
    "get_connection_route",
    "wait_for_events",
    "get_messages_after",
    "export_unread_messages",
];

async fn dispatch(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let state = State::global()?;
    let account = args["account"].as_str();
    match name {
        "list_accounts"=>api.invoke("local.accounts",json!({})).await,
        "check_account_session"=>api.invoke("local.session_health",json!({"config_key":crate::args::text(args,"config_key")?,"config_digest":crate::args::text(args,"config_digest")?})).await,
        "get_file_roots_status"=>Ok(json!({"file_tools_enabled":!state.file_roots().is_empty(),"status":if state.file_roots().is_empty(){"no_allowed_roots"}else{"configured"},"roots":state.file_roots(),"configured_folders":state.file_roots(),"rule":"File operations stay within configured roots. The private state directory and installation outside files/ are protected."})),
        "get_ghost_mode"=>Ok(state.ghost_mode(account,args.get("chat_id"))),
        "set_ghost_mode"=>state.set_ghost_mode(crate::args::boolean(args,"enabled",true)?,account,args.get("chat_id")),
        "safeguard_status"=>Ok(crate::safeguard::bot::status(&state, None)),
        "revoke_always_approval"=>{
            if let Some(folder)=args["folder"].as_str(){return Ok(json!({"revoked":state.revoke_folder(folder)?,"folder":folder}));}
            let tool=crate::args::text(args,"tool")?;
            let chat=crate::args::required(args,"chat_id")?;
            Ok(json!({"revoked":state.revoke_approval(account.unwrap_or("default"),tool,chat)?,"account":account,"tool":tool,"chat_id":chat}))
        }
        "list_contact_aliases"=>{
            let scope=account.context("An explicit account is required for aliases")?;
            let aliases=state.aliases(scope)?;
            let mut grouped=std::collections::BTreeMap::<String,Value>::new();
            for (alias,record) in aliases.as_object().context("Invalid aliases record")? {
                let peer=&record["peer"];
                let id=crate::state::alias_peer_id(peer).or_else(||record.get("id").cloned()).unwrap_or(Value::Null);
                let key=serde_json::to_string(&id)?;
                let row=grouped.entry(key).or_insert_with(||json!({"id":id,"name":record["name"],"peer":peer,"account":scope,"aliases":[],"needs_migration":record["needs_migration"]}));
                if row["name"].is_null(){row["name"]=record["name"].clone();}
                row["aliases"].as_array_mut().expect("Alias list initialized").push(json!(alias));
            }
            Ok(json!(grouped.into_values().collect::<Vec<_>>()))
        }
        "set_contact_alias"=>{
            let scope=account.context("An explicit account is required for aliases")?;
            let alias=crate::args::text(args,"alias")?;
            let input=crate::args::required(args,"chat_id")?;
            let resolved=api.resolve_peer(input).await?;
            state.alias_set(scope,alias,resolved,crate::args::boolean(args,"replace",false)?)
        }
        "delete_contact_alias"=>{
            let scope=account.context("An explicit account is required for aliases")?;
            Ok(json!({"deleted":state.alias_remove(scope,crate::args::text(args,"alias")?)?}))
        }
        "incoming_feed_status"=>state.incoming_feed_status(),
        "disable_incoming_feed"=>state.disable_incoming_feed(),
        "enable_incoming_feed"=>state.enable_incoming_feed(crate::args::limit(args,"settle_ms",6000,50_000)? as u64),
        "wait_for_new_message"|"wait_for_settled_message"=>wait_incoming_with_policy(api,&state,name,args,&crate::server::access::AccessPolicy::default()).await,
        _=>bail!("Unknown local tool"),
    }
}

/// The server passes its scoped policy before the journal selects or consumes
/// events. Trusted direct Rust callers retain the unrestricted wrapper above.
pub(crate) async fn wait_incoming_with_policy(
    api: &dyn Telegram,
    state: &State,
    name: &str,
    args: &Value,
    policy: &crate::server::access::AccessPolicy,
) -> Result<Value> {
    let account = args["account"].as_str();
    let target = if let Some(input) = args.get("chat_id").filter(|v| !v.is_null()) {
        let peer = api.resolve_peer(input).await?;
        Some(marked_peer(&peer)?)
    } else {
        None
    };
    let (timeout, settle) = if name == "wait_for_settled_message" {
        let millis = bounded_number(args, "max_wait_ms", 50_000., 50_000.)?;
        let quiet = bounded_number(args, "settle_ms", 6_000., 50_000.)?;
        (
            Duration::from_secs_f64(millis / 1000.),
            Some(Duration::from_secs_f64(quiet / 1000.)),
        )
    } else {
        (
            Duration::from_secs_f64(bounded_number(args, "timeout", 50., 50.)?),
            None,
        )
    };
    state
        .wait_incoming_permitted(
            account,
            target.as_deref(),
            timeout,
            settle,
            crate::args::limit(args, "limit", 50, 100)? as usize,
            |event| policy.allows_event_peer(&event.chat_id, event.username.as_deref()),
        )
        .await
}

fn bounded_number(args: &Value, key: &str, default: f64, max: f64) -> Result<f64> {
    let value = match args.get(key) {
        None | Some(Value::Null) => default,
        Some(value) => value
            .as_f64()
            .context("A timeout must be a finite number")?,
    };
    if !value.is_finite() || value <= 0. || value > max {
        bail!("{key} must be positive and no greater than {max}");
    }
    Ok(value)
}

pub fn marked_peer(peer: &Value) -> Result<String> {
    match peer["_"].as_str().unwrap_or("") {
        "inputPeerUser" => Ok(normalize_key(&peer["user_id"])),
        "inputPeerChat" => Ok(format!("-{}", normalize_key(&peer["chat_id"]))),
        "inputPeerChannel" => {
            let raw = normalize_key(&peer["channel_id"])
                .parse::<i64>()
                .context("Invalid channel id")?;
            Ok((-1_000_000_000_000i64 - raw).to_string())
        }
        _ => bail!("The selected peer cannot be used as an incoming private chat target"),
    }
}
