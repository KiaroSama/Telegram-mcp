//! Owner-only views and revocation for the existing approval store.
use super::{chat_target, unix_seconds};
use crate::state::State;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MENU: &[(&str, &str)] = &[
    ("pending", "Show pending requests"),
    ("always", "List or revoke permanent chat approvals"),
    ("reset_always", "Remove all permanent chat approvals"),
    ("folders", "List or revoke recorded folder grants"),
    ("status", "Show approval counts"),
    ("bypass", "View or change temporary automatic approval"),
    ("accounts", "Show configured Telegram accounts"),
    ("help", "Show approval bot commands"),
];
const PAGE_SIZE: usize = 8;

pub(super) fn html(text: &str, limit: usize) -> String {
    let mut output = String::new();
    for ch in text.chars().take(limit) {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            ch if ch.is_control()
                || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                output.push(' ')
            }
            _ => output.push(ch),
        }
    }
    output
}

pub(super) fn prompt_text(
    code: &str,
    tool: &str,
    args: &Value,
    reasons: &[String],
    token: &str,
    details: &str,
) -> String {
    let field = |text: &str, limit| html(&text.replace(token, "[REDACTED]"), limit);
    let account = field(args["account"].as_str().unwrap_or("default"), 64);
    let target = field(
        &chat_target(args)
            .map(Value::to_string)
            .unwrap_or_else(|| "No specific chat".to_owned()),
        120,
    );
    let arguments = super::approval_details::prompt_summary(args);
    let summary = field(&arguments, 1400);
    let truncation = if arguments.chars().nth(1400).is_some() {
        "\n(Arguments abbreviated. Use the owner terminal to review the full request.)"
    } else {
        ""
    };
    let details = if details.is_empty() {
        String::new()
    } else {
        format!("\n\n<pre>{}</pre>", field(details, 600))
    };
    format!(
        "🔐 <b>Telegram MCP approval</b>\nAccount: <code>{account}</code>\nTool: <code>{}</code>\nTarget: <code>{target}</code>\nReasons: {}\nRequest: <code>{}</code>\n\n<pre>{summary}</pre>{truncation}{details}",
        field(tool, 100),
        field(&reasons.join(", "), 280),
        field(code, 16)
    )
}

pub(super) fn commands() -> Value {
    json!({"commands":MENU.iter().map(|(command, description)| json!({"command":command,"description":description})).collect::<Vec<_>>(),"scope":{"type":"all_private_chats"}})
}

fn pending(state: &State) -> Result<Vec<Value>> {
    let requests = state.read_json("approval-pending.json", json!({}))?;
    let now = unix_seconds();
    Ok(requests
        .as_object()
        .context("Approval request record is invalid")?
        .values()
        .filter(|entry| {
            entry["expires"]
                .as_u64()
                .is_some_and(|expires| expires >= now)
        })
        .cloned()
        .collect())
}

pub(super) fn message(
    state: &State,
    input: &str,
    username: &str,
    token: &str,
    owner: i64,
    observed: &std::collections::BTreeMap<String, super::channels::ChannelAvailability>,
) -> Option<Value> {
    let field = |value: &str, limit| html(&value.replace(token, "[REDACTED]"), limit);
    let mut words = input.split_whitespace();
    let command = words.next()?.strip_prefix('/')?;
    let command = if let Some((command, target)) = command.split_once('@') {
        if !target.eq_ignore_ascii_case(username) {
            return None;
        }
        command
    } else {
        command
    };
    if command == "bypass" {
        let arg = words
            .next()
            .map(|first| if words.next().is_none() { first } else { "" });
        return Some(super::bot_bypass::command(state, arg, owner));
    }
    if command == "accounts" && words.next().is_none() {
        return Some(super::bot_bypass::accounts(observed));
    }
    let page = words
        .next()
        .and_then(|input| input.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    Some(match command {
        "help" | "start" => text(&format!(
            "🤖 <b>Approval bot commands</b>\n{}",
            MENU.iter()
                .map(|(command, description)| format!("/{command} - {description}"))
                .collect::<Vec<_>>()
                .join("\n")
        )),
        "pending" => match pending(state) {
            Ok(rows) if rows.is_empty() => text("📭 No pending approval requests."),
            Ok(rows) => {
                let mut body = format!("⏳ <b>Pending requests ({})</b>", rows.len());
                for row in rows.iter().take(PAGE_SIZE) {
                    let args = &row["arguments"];
                    let account = field(args["account"].as_str().unwrap_or("default"), 64);
                    let tool = field(row["tool"].as_str().unwrap_or("Unknown tool"), 100);
                    let chat = chat_target(args)
                        .map_or_else(|| "No specific chat".into(), |v| field(&v.to_string(), 120));
                    body.push_str(&format!("\n• {account}: <code>{tool}</code> ({chat})"));
                }
                if rows.len() > PAGE_SIZE {
                    body.push_str("\nUse the original prompts or the owner terminal to review individual requests.");
                }
                text(&body)
            }
            Err(_) => text("The pending approval store is unreadable."),
        },
        "always" => grant_list(
            state,
            "grants",
            "a",
            "always",
            "Permanent chat approvals",
            page,
            token,
        ),
        "folders" => grant_list(
            state,
            "folders",
            "f",
            "folders",
            "Recorded folder grants",
            page,
            token,
        ),
        "reset_always" => {
            json!({"text":"⚠️ Remove every permanent chat approval? This does not change folder grants.","reply_markup":{"inline_keyboard":[[{"text":"🗑️ Remove all chat approvals","callback_data":"mn:ra:y"},{"text":"↩️ Cancel","callback_data":"mn:ra:n"}]]}})
        }
        "status" => {
            let grants = state.always_approvals();
            if !grants["error"].is_null() {
                text("The permanent approval store is unreadable.")
            } else if let Ok(pending) = pending(state) {
                text(&format!(
                    "📊 <b>Approval status</b>\n⏳ Pending requests: {}\n♾️ Permanent chat approvals: {}\n📁 Recorded folder grants: {}",
                    pending.len(),
                    rows(&grants, "grants").len(),
                    rows(&grants, "folders").len()
                ))
            } else {
                text("The pending approval store is unreadable.")
            }
        }
        _ => return None,
    })
}

fn text(body: &str) -> Value {
    json!({"text":body,"parse_mode":"HTML","link_preview_options":{"is_disabled":true}})
}

fn rows<'a>(store: &'a Value, key: &str) -> &'a [Value] {
    store[key].as_array().map_or(&[], Vec::as_slice)
}

fn grant_list(
    state: &State,
    key: &str,
    kind: &str,
    command: &str,
    label: &str,
    page: usize,
    token: &str,
) -> Value {
    let store = state.always_approvals();
    if !store["error"].is_null() {
        return text("The permanent approval store is unreadable.");
    }
    let entries = rows(&store, key);
    if entries.is_empty() {
        return text(&format!("📭 {label}: none."));
    }
    let pages = entries.len().div_ceil(PAGE_SIZE);
    let page = page.min(pages);
    let start = (page - 1) * PAGE_SIZE;
    let mut body = format!("<b>{label}</b> (page {page} of {pages})");
    let mut buttons = Vec::new();
    for (index, row) in entries.iter().skip(start).take(PAGE_SIZE).enumerate() {
        let number = start + index + 1;
        let display = if kind == "a" {
            format!(
                "{}: {} in {}",
                row[0].as_str().unwrap_or(""),
                row[1].as_str().unwrap_or(""),
                row[2].as_str().unwrap_or("")
            )
        } else if let Some(path) = row.as_str() {
            path.to_owned()
        } else {
            continue;
        };
        body.push_str(&format!(
            "\n{number}. <code>{}</code>",
            html(&display.replace(token, "[REDACTED]"), 180)
        ));
        buttons.push([json!({"text":format!("🗑️ Revoke {number}"), "callback_data":format!("mn:rm:{kind}:{}", row_key(row))})]);
    }
    if page < pages {
        body.push_str(&format!("\n➡️ Next page: /{command} {}", page + 1));
    }
    let mut body = text(&body);
    body["reply_markup"] = json!({"inline_keyboard":buttons});
    body
}

fn row_key(row: &Value) -> String {
    crate::lowercase_hex(&Sha256::digest(row.to_string().as_bytes())[..16])
}

pub(super) fn callback(state: &State, data: &str) -> Result<&'static str> {
    let (key, target) = match data {
        "mn:ra:n" => return Ok("↩️ Cancelled"),
        "mn:ra:y" => ("grants", None),
        _ => match data
            .strip_prefix("mn:rm:")
            .and_then(|body| body.split_once(':'))
        {
            Some(("a", id)) => ("grants", Some(id)),
            Some(("f", id)) => ("folders", Some(id)),
            _ => return Ok("Unknown action"),
        },
    };
    let removed = state.update_json(
        "always-approvals.json",
        json!({"grants":[],"folders":[]}),
        |store| {
            let rows = store
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .context("Permanent approval store is invalid")?;
            let before = rows.len();
            rows.retain(|row| target.is_some_and(|id| row_key(row) != id));
            Ok(rows.len() != before)
        },
    )?;
    Ok(if removed {
        "🗑️ Approval removed"
    } else {
        "No matching approval"
    })
}
