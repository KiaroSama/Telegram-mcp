//! Local configuration, event waits and owner-controlled safety settings.
mod accounts;
mod aliases;
mod commands;
mod feed;
mod ghost;
pub mod proxies;
pub(crate) mod roots;
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
        "list_accounts" => accounts::list_accounts(api).await,
        "check_account_session" => accounts::check_account_session(api, args).await,
        "get_file_roots_status" => Ok(roots::get_file_roots_status(&state, None)),
        "get_ghost_mode" => ghost::get_ghost_mode(api, &state, account, args).await,
        "set_ghost_mode" => ghost::set_ghost_mode(api, &state, account, args).await,
        "safeguard_status" => Ok(crate::safeguard::bot::status(
            &state,
            None,
            account,
            Default::default(),
            Default::default(),
        )),
        "revoke_always_approval" => {
            if let Some(folder) = args["folder"].as_str() {
                use crate::safeguard::folders::{FolderGrants, FolderScope};
                let store = FolderGrants::new(&state, state.installation_root())?;
                let account = account.unwrap_or("default");
                let path = std::path::Path::new(folder);
                let mut revoked = false;
                if path.is_absolute() {
                    for scope in [
                        FolderScope::Read,
                        FolderScope::Write,
                        FolderScope::ReadWrite,
                    ] {
                        revoked |= store.revoke(account, path, scope)?;
                    }
                }
                revoked |= state.revoke_folder(folder)?;
                return Ok(json!({"revoked":revoked}));
            }
            let tool = crate::args::text(args, "tool")?;
            let chat = crate::args::required(args, "chat_id")?;
            Ok(
                json!({"revoked":state.revoke_approval(account.unwrap_or("default"),tool,chat)?,"account":account,"tool":tool,"chat_id":chat}),
            )
        }
        "list_contact_aliases" => aliases::list_contact_aliases(
            &state,
            account.context("An explicit account is required for aliases")?,
        ),
        "set_contact_alias" => {
            aliases::set_contact_alias(
                api,
                &state,
                account.context("An explicit account is required for aliases")?,
                args,
            )
            .await
        }
        "delete_contact_alias" => aliases::delete_contact_alias(
            &state,
            account.context("An explicit account is required for aliases")?,
            args,
        ),
        "incoming_feed_status" => feed::incoming_feed_status(&state),
        "disable_incoming_feed" => feed::disable_incoming_feed(&state),
        "enable_incoming_feed" => feed::enable_incoming_feed(&state, args),
        "wait_for_new_message" | "wait_for_settled_message" => {
            wait_incoming_with_policy(
                api,
                &state,
                name,
                args,
                &crate::server::access::AccessPolicy::default(),
            )
            .await
        }
        _ => bail!("Unknown local tool"),
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
