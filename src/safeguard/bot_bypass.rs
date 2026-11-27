use super::{
    bypass::{BypassPreset, BypassState},
    unix_time,
};
use crate::state::State;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

const INSTRUCTIONS: &str = "✏️ Send /bypass 2h30m or /bypass 90m to set a custom time. Send /bypass off to restore prompts.";

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

const BLOCKED: &str = "🛡️ Protected actions remain blocked.";

/// Owner-facing bypass status for the bot (HTML); `describe` stays the JSON form.
pub(super) fn status_html(bypass: BypassState, now: f64) -> String {
    match bypass.effective(now) {
        BypassState::Off => format!("🔒 Bypass mode: <b>OFF</b>\n{BLOCKED}"),
        BypassState::Indefinite { .. } => {
            format!("🔓 Bypass mode: <b>ON</b>\n♾️ Until you turn it off.\n{BLOCKED}")
        }
        BypassState::Timed {
            expires_at_unix, ..
        } => {
            // Round up to the minute so a fresh 30-day preset reads "30 d 0 h 0 min".
            let left = ((expires_at_unix - now).max(0.0).ceil() as u64).div_ceil(60) * 60;
            format!(
                "🔓 Bypass mode: <b>ON</b>\n⏳ For {}.\n{BLOCKED}",
                super::bypass::span(left)
            )
        }
    }
}

pub(super) fn menu(state: &State) -> Value {
    let now = unix_time();
    let status = status_html(BypassState::read(state, now), now);
    let labels = [
        "⏱️ 1 hour",
        "⏱️ 6 hours",
        "📅 1 day",
        "📅 7 days",
        "🗓️ 30 days",
        "♾️ Until I turn it off",
        "✏️ Custom time",
        "🔒 Turn bypass off",
    ];
    let buttons: Vec<_> = BypassPreset::MENU
        .iter()
        .zip(labels)
        .map(|((key, _), label)| json!({"text":label,"callback_data":format!("mn:bp:{key}")}))
        .collect();
    json!({"text":status, "parse_mode":"HTML", "reply_markup":{"inline_keyboard":[&buttons[..3], &buttons[3..5], [&buttons[6]], [&buttons[5]], [&buttons[7]]]}})
}

/// Persist a preset: `Ok(true)` saved, `Ok(false)` needs a custom duration, `Err` is the owner notice.
pub(super) fn apply(state: &State, preset: BypassPreset, owner: i64) -> Result<bool, &'static str> {
    match preset.state(unix_time(), owner) {
        Ok(Some(next)) => next
            .write(state)
            .map(|()| true)
            .map_err(|_| "⚠️ Could not save bypass state. Check /bypass before relying on it."),
        Ok(None) => Ok(false),
        Err(_) => Err(INVALID),
    }
}

const INVALID: &str = "⚠️ Invalid bypass duration. Use /bypass to see valid options.";

pub(super) fn instructions() -> Value {
    json!({"text":INSTRUCTIONS})
}

pub(super) fn command(state: &State, input: Option<&str>, owner: i64) -> Value {
    let Some(input) = input else {
        return menu(state);
    };
    match BypassPreset::parse(input)
        .map_err(|_| INVALID)
        .and_then(|preset| apply(state, preset, owner))
    {
        Ok(true) => menu(state),
        Ok(false) => instructions(),
        Err(notice) => json!({"text":notice}),
    }
}

pub(super) fn accounts(
    observed: &std::collections::BTreeMap<String, super::channels::ChannelAvailability>,
) -> Value {
    use super::channels::ChannelAvailability::{Available, Unavailable};
    let text = match crate::backend::account_labels() {
        Ok(labels) if labels.is_empty() => "👤 No Telegram accounts are configured.".to_owned(),
        Ok(labels) => {
            let rows = labels
                .iter()
                .take(48)
                .map(|label| {
                    let health = match observed.get(label) {
                        Some(Available) => "🟢 connected",
                        Some(Unavailable) => "🔴 unavailable",
                        _ => "⚪ unknown",
                    };
                    format!(
                        "<code>{}</code>: {health}",
                        super::bot_menu::html(label, 64)
                    )
                })
                .collect::<Vec<_>>();
            let remainder = if labels.len() > rows.len() {
                format!("\n{} more configured accounts", labels.len() - rows.len())
            } else {
                String::new()
            };
            format!(
                "👥 <b>Telegram accounts</b>\n{}{remainder}",
                rows.join("\n")
            )
        }
        Err(_) => "⚠️ Configured Telegram accounts could not be read.".to_owned(),
    };
    json!({"text":text,"parse_mode":"HTML","reply_markup":{"inline_keyboard":[[{"text":"🔄 Refresh accounts","callback_data":"mn:ac"}]]}})
}
