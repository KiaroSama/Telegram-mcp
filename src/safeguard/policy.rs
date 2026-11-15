use super::tables;
use crate::state::normalize_key;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Free,
    Gated,
    SeenSignal,
    Send,
}

#[derive(Debug, Default, Clone)]
pub struct Facts {
    pub ghost_on: bool,
    pub first_message: bool,
    pub bulk: bool,
    pub tainted: Vec<Value>,
    pub approval_chats: HashSet<String>,
    pub pending_codes: HashSet<String>,
    pub protected_paths: Vec<PathBuf>,
    pub protected_folder: bool,
    pub outside_folders: Vec<String>,
    pub approval_message: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyDecision {
    pub outcome: &'static str,
    pub reasons: Vec<String>,
    pub tainted: Vec<Value>,
}

pub fn categorize(name: &str, read_only: bool, destructive: bool) -> Category {
    if tables::GATED.contains(&name) {
        Category::Gated
    } else if tables::SEEN_SIGNAL.contains(&name) {
        Category::SeenSignal
    } else if tables::SEND.contains(&name) {
        Category::Send
    } else if tables::FREE_WRITES.contains(&name) || read_only {
        Category::Free
    } else if destructive {
        Category::Gated
    } else {
        Category::Free
    }
}

fn scalars<'a>(value: &'a Value, output: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            for value in map.values() {
                scalars(value, output);
            }
        }
        Value::Array(array) => {
            for value in array {
                scalars(value, output);
            }
        }
        Value::String(_) | Value::Number(_) => output.push(value),
        _ => {}
    }
}

fn chat_key(value: &Value) -> String {
    let raw = normalize_key(value);
    let lower = raw.to_lowercase();
    for prefix in [
        "https://t.me/",
        "http://t.me/",
        "t.me/",
        "https://telegram.me/",
        "telegram.me/",
        "tg://resolve?domain=",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return rest
                .split(['?', '/', '&', '#'])
                .next()
                .unwrap_or("")
                .to_owned();
        }
    }
    lower
}

pub fn decide(
    name: &str,
    read_only: bool,
    destructive: bool,
    args: &Value,
    facts: &Facts,
) -> PolicyDecision {
    let refusal = |reason: &str| PolicyDecision {
        outcome: "refuse",
        reasons: vec![reason.to_owned()],
        tainted: vec![],
    };
    let mut values = Vec::new();
    scalars(args, &mut values);
    if facts.approval_message
        || values.iter().any(|value| {
            facts.approval_chats.contains(&chat_key(value))
                || value.as_str().is_some_and(|s| {
                    facts.pending_codes.iter().any(|code| {
                        s.split(|c: char| !c.is_ascii_alphanumeric())
                            .any(|token| token.eq_ignore_ascii_case(code))
                    })
                })
        })
    {
        return refusal("touches_approval_channel");
    }
    if values.iter().filter_map(|v| v.as_str()).any(|text| {
        if text
            .replace('\\', "/")
            .to_lowercase()
            .contains("telegram_mcp/safeguard")
        {
            return !facts.protected_paths.is_empty();
        }
        // Preserve native Windows verbatim prefixes. Changing backslashes to
        // slashes before constructing a path changes the prefix's meaning.
        let path = PathBuf::from(text);
        let absolute = if path.is_absolute() {
            path
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        };
        let resolved = crate::state::canonical_existing(&absolute).unwrap_or(absolute);
        facts.protected_paths.iter().any(|root| {
            resolved.starts_with(
                crate::state::canonical_existing(root).unwrap_or_else(|_| root.clone()),
            )
        })
    }) {
        return refusal("touches_safeguard_files");
    }
    if facts.protected_folder {
        return refusal("touches_protected_folder");
    }
    let category = categorize(name, read_only, destructive);
    let mut reasons = Vec::new();
    if category == Category::Gated {
        reasons.push("gated".to_owned());
    }
    if category == Category::SeenSignal && facts.ghost_on {
        reasons.push("seen_signal_under_ghost".to_owned());
    }
    if name == "set_ghost_mode" && args.get("enabled") == Some(&json!(false)) {
        reasons.push("ghost_off".to_owned());
    }
    if category == Category::Send {
        if facts.first_message {
            reasons.push("first_message".to_owned());
        }
        if facts.bulk {
            reasons.push("bulk_send".to_owned());
        }
    }
    let tainted = if read_only {
        vec![]
    } else {
        facts.tainted.clone()
    };
    if !tainted.is_empty() {
        reasons.push("tainted".to_owned());
    }
    if !facts.outside_folders.is_empty() {
        reasons.push("outside_project_folder".to_owned());
    }
    PolicyDecision {
        outcome: if reasons.is_empty() { "run" } else { "ask" },
        reasons,
        tainted,
    }
}
