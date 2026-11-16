use regex::Regex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;
#[cfg(test)]
#[path = "../../tests/unit/taint.rs"]
mod tests;

const PASSAGE: usize = 24;
const MAX_WINDOWS: usize = 300_000;
const MAX_FRAGMENTS: usize = 5_000;

#[derive(Default)]
struct Account {
    fragments: VecDeque<(String, String, Value)>,
    windows: HashMap<String, (Value, usize)>,
    batches: VecDeque<Vec<String>>,
    total: usize,
}

#[derive(Default)]
pub(super) struct Memory {
    accounts: HashMap<String, Account>,
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn windows(text: &str) -> Vec<String> {
    let chars: Vec<_> = normalized(text).chars().take(4_000).collect();
    if chars.len() < PASSAGE {
        return vec![];
    }
    chars
        .windows(PASSAGE)
        .map(|window| window.iter().collect())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn fragments(text: &str) -> Vec<(String, String)> {
    static PATTERNS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        vec![
            (
                "url",
                Regex::new(r"(?i)(?:https?://|www\.|tg://|(?:telegram|t)\.me/)\S+")
                    .expect("URL pattern"),
            ),
            (
                "username",
                Regex::new(r"@[A-Za-z][A-Za-z0-9_]{3,31}\b").expect("Username pattern"),
            ),
            (
                "phone",
                Regex::new(r"\+?\d[\d\s()\-]{6,}\d").expect("Phone pattern"),
            ),
        ]
    });
    let mut found = Vec::new();
    for (kind, pattern) in patterns {
        for item in pattern.find_iter(text) {
            if *kind == "username"
                && item.start() > 0
                && text[..item.start()]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '@')
            {
                continue;
            }
            let value = if *kind == "phone" {
                item.as_str()
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect::<String>()
            } else if *kind == "username" {
                item.as_str().trim_start_matches('@').to_lowercase()
            } else {
                item.as_str()
                    .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"'])
                    .to_lowercase()
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_start_matches("www.")
                    .to_owned()
            };
            if *kind == "phone" && value.len() < 8 {
                continue;
            }
            found.push((kind.to_string(), value));
        }
    }
    found
}

impl Memory {
    fn note_text(&mut self, account: &str, chat: &Value, text: &str) {
        if text.is_empty() {
            return;
        }
        let memory = self.accounts.entry(account.to_lowercase()).or_default();
        for (kind, value) in fragments(text) {
            memory
                .fragments
                .retain(|(old_kind, old_value, _)| *old_kind != kind || *old_value != value);
            memory.fragments.push_back((kind, value, chat.clone()));
            while memory.fragments.len() > MAX_FRAGMENTS {
                memory.fragments.pop_front();
            }
        }
        let batch = windows(text);
        if batch.is_empty() {
            return;
        }
        for window in &batch {
            let record = memory
                .windows
                .entry(window.clone())
                .or_insert_with(|| (chat.clone(), 0));
            record.0 = chat.clone();
            record.1 += 1;
        }
        memory.total += batch.len();
        memory.batches.push_back(batch);
        while memory.total > MAX_WINDOWS {
            if let Some(old) = memory.batches.pop_front() {
                memory.total -= old.len();
                for window in old {
                    if let Some(record) = memory.windows.get_mut(&window) {
                        record.1 -= 1;
                        if record.1 == 0 {
                            memory.windows.remove(&window);
                        }
                    }
                }
            } else {
                break;
            }
        }
    }

    pub fn note_value(&mut self, account: &str, chat: &Value, value: &Value) {
        match value {
            Value::String(text) => self.note_text(account, chat, text),
            Value::Array(rows) => {
                for row in rows {
                    self.note_value(account, chat, row);
                }
            }
            Value::Object(map) => {
                if map.get("out").and_then(Value::as_bool) == Some(true)
                    && map.get("fwd_from").is_none_or(Value::is_null)
                {
                    return;
                }
                for (key, value) in map {
                    if [
                        "message",
                        "text",
                        "title",
                        "description",
                        "name",
                        "full_name",
                        "sender_name",
                        "sender",
                        "first_name",
                        "last_name",
                        "caption",
                        "filename",
                        "file_name",
                        "url",
                        "phone",
                        "about",
                        "bio",
                        "quote",
                        "quote_text",
                        "summary",
                        "link_text",
                        "alt",
                        "body",
                        "author",
                        "message_text",
                    ]
                    .contains(&key.as_str())
                    {
                        self.note_value(account, chat, value);
                    } else if key == "topics" {
                        if let Some(names) = value.as_object() {
                            for name in names.values() {
                                self.note_value(account, chat, name);
                            }
                        }
                    } else if key == "username" {
                        if let Some(name) = value.as_str() {
                            self.note_text(account, chat, &format!("@{name}"));
                        }
                    } else if value.is_array() || value.is_object() {
                        self.note_value(account, chat, value);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn find(&self, account: &str, args: &Value) -> Vec<Value> {
        let Some(memory) = self.accounts.get(&account.to_lowercase()) else {
            return vec![];
        };
        let mut strings = Vec::new();
        collect_strings(args, &mut strings);
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        for text in strings {
            for (kind, value) in fragments(text) {
                if let Some((_, _, chat)) = memory
                    .fragments
                    .iter()
                    .find(|(k, v, _)| *k == kind && *v == value)
                {
                    let key = format!("{kind}:{value}");
                    if seen.insert(key) {
                        matches.push(json!({"kind":kind,"source_chat":chat}));
                    }
                }
            }
            for window in windows(text) {
                if let Some((chat, _)) = memory.windows.get(&window) {
                    if seen.insert(format!("passage:{chat}")) {
                        matches.push(json!({"kind":"passage","source_chat":chat}));
                    }
                    break;
                }
            }
        }
        matches
    }
}

fn collect_strings<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => strings.push(text),
        Value::Array(rows) => {
            for row in rows {
                collect_strings(row, strings);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_strings(value, strings);
            }
        }
        _ => {}
    }
}
