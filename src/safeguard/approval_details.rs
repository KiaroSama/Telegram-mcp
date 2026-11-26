use super::sealed::SealedStore;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const MAX_ENTRIES: usize = 6;
const MAX_ITEMS: usize = 24;
const MAX_CHARS: usize = 500;
const MAX_DEPTH: usize = 4;

#[derive(Clone, PartialEq, Serialize)]
pub struct ApprovalDetailEntry {
    pub label: String,
    pub target: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub unavailable: bool,
    pub truncated: bool,
}

/// `target` and `before` come from a verified lookup. Never supply raw transport errors.
pub struct DetailInput<'a> {
    pub target: &'a str,
    pub before: Option<&'a Value>,
    pub arguments: &'a Value,
}

type Builder = for<'a> fn(DetailInput<'a>) -> Vec<ApprovalDetailEntry>;

pub struct ApprovalDetails {
    builders: BTreeMap<&'static str, Builder>,
}

impl Default for ApprovalDetails {
    fn default() -> Self {
        let mut registry = Self {
            builders: BTreeMap::new(),
        };
        for tool in [
            "set_default_chat_permissions",
            "set_group_permissions",
            "set_member_exception",
            "remove_member_exception",
        ] {
            registry.register(tool, permissions);
        }
        for tool in [
            "promote_admin",
            "promote_admin_channel",
            "promote_admin_group",
            "promote_admin_community",
            "edit_admin_rights",
            "edit_admin_rights_channel",
            "edit_admin_rights_group",
            "edit_admin_rights_community",
            "demote_admin",
        ] {
            registry.register(tool, admin_rights);
        }
        for tool in [
            "configure_secret_backend",
            "set_secret_chat_timer",
            "set_secret_chats_only_device",
            "set_authorization_secret_chats",
            "clear_secret_history",
            "delete_secret_chat",
            "delete_secret_chat_both_sides",
        ] {
            registry.register(tool, secret_settings);
        }
        registry
    }
}

impl ApprovalDetails {
    pub fn register(&mut self, tool: &'static str, builder: Builder) {
        self.builders.insert(tool, builder);
    }

    pub fn detail_for(
        &self,
        tool: &str,
        input: DetailInput<'_>,
        seals: &SealedStore,
    ) -> Vec<ApprovalDetailEntry> {
        let Some(builder) = self.builders.get(tool) else {
            return Vec::new();
        };
        let target = input.target;
        let mut entries = builder(input);
        if entries.is_empty() {
            entries.push(ApprovalDetailEntry {
                label: "Details".to_owned(),
                target: target.to_owned(),
                before: None,
                after: None,
                unavailable: true,
                truncated: false,
            });
        }
        let exceeded = entries.len() > MAX_ENTRIES;
        entries.truncate(MAX_ENTRIES);
        let mut budget = MAX_CHARS;
        for entry in &mut entries {
            entry.unavailable |= entry.before.is_none();
            entry.label = display_text(
                &seals.redact_text(&entry.label),
                &mut budget,
                &mut entry.truncated,
            );
            entry.target = display_text(
                &seals.redact_text(&entry.target),
                &mut budget,
                &mut entry.truncated,
            );
            for value in [&mut entry.before, &mut entry.after] {
                if let Some(raw) = value {
                    let mut items = MAX_ITEMS;
                    let redacted = seals.redact_value(raw);
                    entry.truncated |= redacted != *raw;
                    let cleaned = clean(&redacted, 0, &mut items, &mut entry.truncated);
                    let cleaned = seals.redact_value(&cleaned);
                    let size = cleaned.to_string().chars().count();
                    if size > budget {
                        *value = None;
                        entry.truncated = true;
                    } else {
                        budget -= size;
                        *raw = cleaned;
                    }
                }
            }
        }
        if exceeded && let Some(last) = entries.last_mut() {
            last.truncated = true;
        }
        entries
    }
}

fn permissions(input: DetailInput<'_>) -> Vec<ApprovalDetailEntry> {
    projected(
        input,
        "Permissions",
        &["permissions", "allowed", "banned_rights"],
        |key| {
            key.starts_with("send_")
                || [
                    "view_messages",
                    "photos",
                    "video_files",
                    "video_messages",
                    "music",
                    "voice_messages",
                    "files",
                    "stickers_and_gifs",
                    "polls",
                    "reactions",
                    "add_members",
                    "create_topics",
                    "edit_own_tags",
                    "change_group_info",
                    "change_info",
                    "invite_users",
                    "pin_messages",
                    "manage_topics",
                    "embed_links",
                    "until_date",
                ]
                .contains(&key)
        },
    )
}

fn admin_rights(input: DetailInput<'_>) -> Vec<ApprovalDetailEntry> {
    projected(input, "Admin rights", &["rights", "admin_rights"], |key| {
        [
            "change_info",
            "post_messages",
            "edit_messages",
            "delete_messages",
            "ban_users",
            "invite_users",
            "pin_messages",
            "add_admins",
            "anonymous",
            "manage_call",
            "other",
            "manage_topics",
            "post_stories",
            "edit_stories",
            "delete_stories",
            "manage_direct_messages",
            "manage_ranks",
        ]
        .contains(&key)
    })
}

fn secret_settings(input: DetailInput<'_>) -> Vec<ApprovalDetailEntry> {
    projected(input, "Secret Chat settings", &[], |key| {
        [
            "enabled",
            "seconds",
            "ttl",
            "ttl_seconds",
            "only_device",
            "accept",
            "delete_both_sides",
            "revoke",
            "mode",
        ]
        .contains(&key)
    })
}

fn projected(
    input: DetailInput<'_>,
    label: &str,
    containers: &[&str],
    accepts: fn(&str) -> bool,
) -> Vec<ApprovalDetailEntry> {
    fn fields(value: &Value, containers: &[&str], accepts: fn(&str) -> bool) -> Option<Value> {
        let map = containers
            .iter()
            .find_map(|key| value.get(key).and_then(Value::as_object))
            .or_else(|| value.as_object())?;
        Some(Value::Object(
            map.iter()
                .filter(|(key, _)| accepts(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ))
    }
    let before = input
        .before
        .and_then(|value| fields(value, containers, accepts));
    let after = fields(input.arguments, containers, accepts);
    let unavailable = before
        .as_ref()
        .and_then(Value::as_object)
        .is_none_or(Map::is_empty);
    vec![ApprovalDetailEntry {
        label: label.to_owned(),
        target: input.target.to_owned(),
        before: before.filter(|_| !unavailable),
        after,
        unavailable,
        truncated: false,
    }]
}

pub(super) fn prompt_summary(value: &Value) -> String {
    let mut budget = MAX_ITEMS;
    let mut truncated = false;
    let mut summary = clean(value, 0, &mut budget, &mut truncated).to_string();
    if truncated {
        summary.push_str(" (abbreviated)");
    }
    summary
}

pub(super) fn prompt_lines(entries: &[ApprovalDetailEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let before = entry
                .before
                .as_ref()
                .map_or_else(|| "unavailable".to_owned(), Value::to_string);
            let after = entry
                .after
                .as_ref()
                .map_or_else(|| "unavailable".to_owned(), Value::to_string);
            let suffix = if entry.truncated { " (truncated)" } else { "" };
            format!("{}: {before} -> {after}{suffix}", entry.label)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "password",
        "credential",
        "session",
        "secret",
        "cookie",
        "api_key",
        "api_hash",
        "access_hash",
        "auth_key",
        "private_key",
        "authorization",
        "approval_code",
        "passphrase",
        "path",
        "directory",
    ]
    .iter()
    .any(|word| key.contains(word))
}

fn private_text(text: &str) -> bool {
    text.contains(['/', '\\'])
        || [".env", "secrets.md", ".session", "bearer "]
            .iter()
            .any(|word| text.to_ascii_lowercase().contains(word))
}

fn display_text(text: &str, budget: &mut usize, truncated: &mut bool) -> String {
    let mut result = String::new();
    let mut word_space = false;
    if private_text(text) {
        *truncated = true;
        return String::new();
    }
    for ch in text.chars().filter(|ch| !matches!(ch, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        if ch.is_whitespace() || ch.is_control() {
            word_space = !result.is_empty();
            continue;
        }
        let cost = 1 + usize::from(word_space);
        if *budget < cost {
            *truncated = true;
            break;
        }
        if word_space { result.push(' '); }
        result.push(ch);
        word_space = false;
        *budget -= cost;
   }
    result
}

fn clean(value: &Value, depth: usize, budget: &mut usize, truncated: &mut bool) -> Value {
    if *budget == 0 || depth > MAX_DEPTH {
        *truncated = true;
        return Value::Null;
    }
    *budget -= 1;
    match value {
        Value::Object(map) => {
            let mut result = Map::new();
            for (key, value) in map {
                if *budget == 0 {
                    *truncated = true;
                    break;
                }
                if sensitive_key(key)
                    || key.len() > 64
                    || !key
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || b"_-.".contains(&ch))
                {
                    *truncated = true;
                    continue;
                }
                result.insert(key.clone(), clean(value, depth + 1, budget, truncated));
            }
            Value::Object(result)
        }
        Value::Array(items) => {
            let mut result = Vec::new();
            for item in items {
                if *budget == 0 {
                    *truncated = true;
                    break;
                }
                result.push(clean(item, depth + 1, budget, truncated));
            }
            Value::Array(result)
        }
        Value::String(text) => {
            let mut limit = 120;
            json!(display_text(text, &mut limit, truncated))
        }
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use anyhow::Result;
    use std::sync::Arc;

    fn fixture() -> Result<(tempfile::TempDir, SealedStore)> {
        let root = tempfile::tempdir()?;
        let state = Arc::new(State::at(root.path().to_owned(), vec![])?);
        state.write_json("sealed.json", &json!({"codes":["ABCD"],"accounts":{}}))?;
        Ok((root, SealedStore::new(state)))
    }

    #[test]
    fn shows_observed_permissions_and_reports_an_unavailable_before() -> Result<()> {
        let (_root, seals) = fixture()?;
        let details = ApprovalDetails::default();
        let arguments = json!({"permissions":{"send_messages":false},"raw_token":"ignored"});
        let observed = json!({"permissions":{"send_messages":true}});
        let make = |before| DetailInput {
            target: "Group 12",
            before,
            arguments: &arguments,
        };
        let found = details.detail_for("set_group_permissions", make(Some(&observed)), &seals);
        assert_eq!(found[0].target, "Group 12");
        assert_eq!(found[0].before, Some(json!({"send_messages":true})));
        assert_eq!(found[0].after, Some(json!({"send_messages":false})));
        assert!(!found[0].unavailable);
        let missing = details.detail_for("set_group_permissions", make(None), &seals);
        assert!(missing[0].unavailable);
        assert!(missing[0].before.is_none());
        assert!(details.detail_for("get_me", make(None), &seals).is_empty());
        Ok(())
    }

    #[test]
    fn custom_builders_cannot_display_nested_credentials_paths_or_sealed_codes() -> Result<()> {
        let (_root, seals) = fixture()?;
        let mut details = ApprovalDetails::default();
        details.register("test", |input| {
            vec![ApprovalDetailEntry {
                label: "Delta\nline".to_owned(),
                target: input.target.to_owned(),
                before: None,
                after: Some(input.arguments.clone()),
                unavailable: false,
                truncated: false,
            }]
        });
        let arguments = json!({"nested":[{"token":"do not show","note":"go ABCD now","other":"/private/location"}],"text":"界".repeat(130),"quote":format!("{}Approval ABCD: sealed body from another device.", "x".repeat(103))});
        let found = details.detail_for(
            "test",
            DetailInput {
                target: "ABCD",
                before: None,
                arguments: &arguments,
            },
            &seals,
        );
        assert_eq!(found[0].label, "Delta line");
        assert_eq!(found[0].target, "[hidden]");
        assert_eq!(
            found[0].after.as_ref().unwrap()["nested"][0],
            json!({"note":"go [hidden] now","other":""})
        );
        assert!(found[0].unavailable && found[0].truncated);
        assert_eq!(
            found[0].after.as_ref().unwrap()["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            120
        );
        let quote = found[0].after.as_ref().unwrap()["quote"].as_str().unwrap();
        assert_eq!(quote, format!("{}[approval request", "x".repeat(103)));
        Ok(())
    }

    #[test]
    fn bounds_builder_entries_and_falls_back_for_an_empty_registered_builder() -> Result<()> {
        let (_root, seals) = fixture()?;
        let mut details = ApprovalDetails::default();
        details.register("empty", |_| vec![]);
        details.register("large", |_| {
            vec![
                ApprovalDetailEntry {
                    label: "x".repeat(501),
                    target: "target".to_owned(),
                    before: Some(json!(1)),
                    after: Some(json!(2)),
                    unavailable: false,
                    truncated: false
                };
                9
            ]
        });
        let input = || DetailInput {
            target: "Group",
            before: None,
            arguments: &Value::Null,
        };
        assert!(details.detail_for("empty", input(), &seals)[0].unavailable);
        let found = details.detail_for("large", input(), &seals);
        assert_eq!(found.len(), MAX_ENTRIES);
        assert_eq!(found[0].label.len(), MAX_CHARS);
        assert!(
            found[0].before.is_none() && found[0].truncated && found[MAX_ENTRIES - 1].truncated
        );
        Ok(())
    }
}
