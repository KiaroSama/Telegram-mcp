use super::{
    bypass::{BypassPreset, BypassState},
    unix_time,
};
use crate::state::State;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

const INSTRUCTIONS: &str =
    "Send /bypass 2h30m or /bypass 90m to set a custom time. Send /bypass off to restore prompts.";

#[derive(Default)]
pub(super) struct OwnerMenus(HashMap<i64, (i64, BTreeSet<String>)>);

impl OwnerMenus {
    pub(super) fn record(&mut self, owner: i64, message_id: i64, body: &Value) {
        let Some(rows) = body["reply_markup"]["inline_keyboard"].as_array() else {
            return;
        };
        let callbacks = rows
            .iter()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|button| button["callback_data"].as_str())
            .filter(|data| valid_callback(data))
            .map(str::to_owned)
            .collect();
        self.0.insert(owner, (message_id, callbacks));
    }

    pub(super) fn consume(&mut self, owner: i64, message_id: i64, data: &str) -> bool {
        if self
            .0
            .get(&owner)
            .is_some_and(|(id, allowed)| *id == message_id && allowed.contains(data))
        {
            self.0.remove(&owner);
            return true;
        }
        false
    }
}

pub(super) fn valid_callback(data: &str) -> bool {
    (1..=64).contains(&data.len()) && data.is_ascii()
}

pub(super) fn menu(state: &State) -> Value {
    let now = unix_time();
    let status = BypassState::read(state, now).describe(now);
    let labels = [
        "1 hour",
        "6 hours",
        "1 day",
        "7 days",
        "30 days",
        "Until I turn it off",
        "Custom time",
        "Turn bypass off",
    ];
    let buttons: Vec<_> = BypassPreset::MENU
        .iter()
        .zip(labels)
        .map(|((key, _), label)| json!({"text":label,"callback_data":format!("mn:bp:{key}")}))
        .collect();
    json!({"text":format!("Approval bypass: {status}. Protected actions remain blocked."), "reply_markup":{"inline_keyboard":[&buttons[..3], &buttons[3..5], [&buttons[6]], [&buttons[5]], [&buttons[7]]]}})
}

pub(super) fn change(state: &State, preset: BypassPreset, owner: i64) -> String {
    let now = unix_time();
    match preset.state(now, owner) {
        Ok(Some(next)) if next.write(state).is_ok() => {
            format!("Approval bypass: {}.", next.describe(now))
        }
        Ok(Some(_)) => {
            "Could not save bypass state. Check /bypass before relying on it.".to_owned()
        }
        Ok(None) => INSTRUCTIONS.to_owned(),
        Err(_) => "Invalid bypass duration. Use /bypass to see valid options.".to_owned(),
    }
}

pub(super) fn command(state: &State, input: Option<&str>, owner: i64) -> Value {
    let Some(input) = input else {
        return menu(state);
    };
    let notice = BypassPreset::parse(input).map_or_else(
        |_| "Invalid bypass duration. Use /bypass to see valid options.".to_owned(),
        |preset| change(state, preset, owner),
    );
    json!({"text":notice})
}

pub(super) fn accounts(
    observed: &std::collections::BTreeMap<String, super::channels::ChannelAvailability>,
) -> Value {
    use super::channels::ChannelAvailability::{Available, Unavailable};
    let text = match crate::backend::account_labels() {
        Ok(labels) if labels.is_empty() => "No Telegram accounts are configured.".to_owned(),
        Ok(labels) => {
            let rows = labels
                .iter()
                .take(48)
                .map(|label| {
                    let health = match observed.get(label) {
                        Some(Available) => "connected",
                        Some(Unavailable) => "unavailable",
                        _ => "unknown",
                    };
                    format!("{label}: {health}")
                })
                .collect::<Vec<_>>();
            let remainder = if labels.len() > rows.len() {
                format!("\n{} more configured accounts", labels.len() - rows.len())
            } else {
                String::new()
            };
            format!("Telegram accounts\n{}{remainder}", rows.join("\n"))
        }
        Err(_) => "Configured Telegram accounts could not be read.".to_owned(),
    };
    json!({"text":text,"reply_markup":{"inline_keyboard":[[{"text":"Refresh accounts","callback_data":"mn:ac"}]]}})
}
