use super::{bot, bypass, channels, folders, unix_time};
use crate::state::State;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn status_value(
    state: &State,
    account: Option<&str>,
    mut remembered: std::collections::BTreeMap<String, usize>,
    mut observed: std::collections::BTreeMap<String, channels::ChannelAvailability>,
    config: Option<&bot::Config>,
) -> Value {
    let account = account.map(str::to_ascii_lowercase);
    let matches_account = |label: &str| account.as_deref().is_none_or(|wanted| wanted == label);
    let grants = state.always_approvals();
    let tool_grants: Vec<_> = grants["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row[0].as_str().is_some_and(matches_account))
        .collect();
    let folder_grants =
        folders::FolderGrants::new(state, state.installation_root()).and_then(|store| store.list());
    let folder_error = folder_grants.is_err();
    let paths = folder_grants
        .unwrap_or_default()
        .into_iter()
        .filter(|row| matches_account(&row.account))
        .map(|row| {
            let reference = crate::lowercase_hex(&Sha256::digest(
                row.directory.as_os_str().as_encoded_bytes(),
            ));
            json!({"account":row.account,"directory_ref":reference,"scope":row.scope})
        })
        .collect::<Vec<_>>();
    let known = account
        .iter()
        .cloned()
        .chain(crate::backend::account_labels().unwrap_or_default())
        .chain(
            grants["grants"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row[0].as_str().map(str::to_owned)),
        );
    for label in known {
        remembered.entry(label.clone()).or_default();
        observed.entry(label).or_default();
    }
    remembered.retain(|label, _| matches_account(label));
    observed.retain(|label, _| matches_account(label));
    let now = unix_time();
    let bypass = bypass::BypassState::read(state, now);
    let (mode, until) = match bypass {
        bypass::BypassState::Off => ("off", None),
        bypass::BypassState::Indefinite { .. } => ("indefinite", None),
        bypass::BypassState::Timed {
            expires_at_unix, ..
        } => ("timed", Some(expires_at_unix)),
    };
    let folder_refs: Vec<_> = paths.iter().map(|row| &row["directory_ref"]).collect();
    json!({"installed":true,"bypass":bypass.is_active(now),"bypass_description":bypass.describe(now),"bypass_until":until,
        "bypass_state":{"mode":mode,"expires_at_unix":until},"secret_chat_integration":"native",
        "channels":{"owner_terminal":"available through telegram-mcp approve",
            "telegram_bot":config.map(bot::Config::status).unwrap_or_else(|| json!("not configured")),
            "dialog":"unknown","saved_messages":observed},
        "approval_timeout_seconds":config.map_or(bot::DEFAULT_TIMEOUT, |config| config.timeout.as_secs()),
        "approval_grant_seconds":60,"always_approved":tool_grants,"always_allowed_folders":folder_refs,
        "folders":{"free_count":state.free_folders().len(),"configured_root_count":state.file_roots().len(),"client_root_count":null,"grants":paths},
        "grant_error":if folder_error {json!("Folder grants could not be read")} else {grants["error"].clone()},
        "bulk_send":{"chats":5,"within_seconds":60},"untrusted_content":{"passage_characters":24,"remembered":remembered}})
}
