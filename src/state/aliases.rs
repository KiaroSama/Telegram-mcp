use super::{State, normalize_key};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use unicode_normalization::UnicodeNormalization;

pub fn alias_key(alias: &str) -> String {
    alias
        .nfc()
        .collect::<String>()
        .trim()
        .trim_start_matches('@')
        .replace('ё', "е")
        .replace('Ё', "Е")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn identifier_like(alias: &str) -> bool {
    let value = alias.trim();
    let bare = value.trim_start_matches('@');
    let lower = bare.to_lowercase();
    if matches!(lower.as_str(), "me" | "self" | "saved" | "saved messages")
        || lower.contains("t.me/")
        || lower.starts_with("tg://")
    {
        return true;
    }
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || "+-() ".contains(c))
    {
        return true;
    }
    bare.len() >= 5 && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn alias_peer_id(peer: &Value) -> Option<Value> {
    let number = |value: &Value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
    };
    if peer.is_number() || peer.is_string() {
        return number(peer).map(|id| json!(id.to_string()));
    }
    match peer["_"].as_str() {
        Some("inputPeerChat") => number(&peer["chat_id"])
            .and_then(i64::checked_neg)
            .map(|id| json!(id.to_string())),
        Some("inputPeerChannel") => number(&peer["channel_id"])
            .and_then(|id| id.checked_add(1_000_000_000_000))
            .and_then(i64::checked_neg)
            .map(|id| json!(id.to_string())),
        Some("inputPeerSelf") => Some(json!("me")),
        _ => number(&peer["user_id"]).map(|id| json!(id.to_string())),
    }
}

impl State {
    fn migrate_aliases(&self) -> Result<()> {
        let _lock = self.state_lock()?;
        let existing = self.read_json_unlocked("aliases-v2.json", Value::Null)?;
        if !existing.is_null() {
            if !existing.is_object() || !existing["accounts"].is_object() {
                bail!("Alias store is invalid; existing data was preserved");
            }
            return Ok(());
        }
        let source = if let Some(path) = &self.legacy_alias_source {
            Some(path.clone())
        } else if self.root.join("aliases.json").exists() {
            Some(self.root.join("aliases.json"))
        } else if self.installation.join("aliases.json").exists() {
            Some(self.installation.join("aliases.json"))
        } else {
            None
        };
        let mut migrated = json!({"accounts":{},"legacy_unscoped":{}});
        if let Some(source) = source {
            let meta = source
                .symlink_metadata()
                .context("Cannot inspect legacy alias source; existing data was preserved")?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 16 * 1024 * 1024 {
                bail!("Legacy alias source must be a bounded regular file");
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&source)
                .context("Cannot read legacy alias source; existing data was preserved")?;
            let mut bytes = Vec::new();
            std::io::Read::take(&mut file, 16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 16 * 1024 * 1024 {
                bail!("Legacy alias source exceeds its byte budget");
            }
            let text = std::str::from_utf8(&bytes)
                .context("Legacy alias source must be UTF-8; existing bytes were preserved")?;
            let old: Value = serde_json::from_str(text)
                .context("Legacy alias source is corrupt; existing bytes were preserved")?;
            let rows = old
                .as_object()
                .context("Legacy alias source must be an object")?;
            let sole = crate::account_manager::current_config()
                .ok()
                .and_then(|snapshot| {
                    let labels = snapshot
                        .values
                        .iter()
                        .filter(|(_, value)| !value.is_empty())
                        .filter_map(|(key, _)| crate::account_manager::label_for(key).ok())
                        .collect::<std::collections::BTreeSet<_>>();
                    if labels.len() == 1 {
                        labels.into_iter().next()
                    } else {
                        None
                    }
                });
            for (key, record) in rows {
                let (scoped, alias) = key
                    .split_once('\n')
                    .map(|(scope, alias)| (Some(scope.to_lowercase()), alias))
                    .unwrap_or((None, key.as_str()));
                let recorded = record["account"].as_str().map(str::to_lowercase);
                if scoped
                    .as_ref()
                    .zip(recorded.as_ref())
                    .is_some_and(|(left, right)| left != right)
                {
                    bail!("Legacy alias account scope conflicts; no migration was written");
                }
                let account = scoped.or(recorded).or_else(|| sole.clone());
                let alias = alias_key(alias);
                if alias.is_empty() {
                    bail!("Legacy alias has an empty normalized key");
                }
                let recipient = if record.is_object() {
                    &record["id"]
                } else {
                    record
                };
                let id = recipient
                    .as_i64()
                    .or_else(|| recipient.as_str().and_then(|id| id.parse::<i64>().ok()))
                    .filter(|id| *id != 0)
                    .context(
                        "Legacy alias has an invalid recipient id; no migration was written",
                    )?;
                let migrated_record = json!({"peer":id.to_string(),"id":id.to_string(),"name":record["name"],"account":account,"needs_migration":account.is_none()});
                if let Some(account) = account {
                    crate::session::validate_label(&account)?;
                    if migrated["accounts"][&account].is_null() {
                        migrated["accounts"][&account] = json!({});
                    }
                    if !migrated["accounts"][&account][&alias].is_null() {
                        bail!("Normalized legacy aliases collide; confirm them before migration");
                    }
                    migrated["accounts"][&account][&alias] = migrated_record;
                } else {
                    if !migrated["legacy_unscoped"][&alias].is_null() {
                        bail!("Normalized legacy aliases collide; confirm them before migration");
                    }
                    migrated["legacy_unscoped"][&alias] = migrated_record;
                }
            }
            migrated["legacy_source_digest"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        }
        self.write_json_unlocked("aliases-v2.json", &migrated)
    }

    pub fn aliases(&self, account: &str) -> Result<Value> {
        self.migrate_aliases()?;
        let store = self.read_json("aliases-v2.json", json!({"accounts":{}}))?;
        if !store.is_object() || !store["accounts"].is_object() {
            bail!("Alias store is invalid; existing data was preserved");
        }
        let mut rows = store["accounts"]
            .get(account.to_lowercase())
            .cloned()
            .unwrap_or(json!({}));
        if !rows.is_object() {
            bail!("Account alias store is invalid");
        }
        if let Some(unscoped) = store["legacy_unscoped"].as_object() {
            let own = rows.as_object_mut().expect("Alias object validated");
            for (alias, record) in unscoped {
                own.entry(alias.clone()).or_insert_with(|| record.clone());
            }
        }
        Ok(rows)
    }

    pub fn alias_lookup(&self, account: &str, alias: &str) -> Result<Option<Value>> {
        if identifier_like(alias) {
            return Ok(None);
        }
        Ok(self
            .aliases(account)?
            .get(alias_key(alias))
            .filter(|record| {
                record["needs_migration"] != json!(true)
                    && record["account"].as_str() == Some(&account.to_lowercase())
            })
            .cloned())
    }

    /// Similar aliases are suggestions only; lookup and mutation remain exact.
    pub fn alias_matches(&self, account: &str, query: &str) -> Result<Value> {
        let query = alias_key(query);
        if query.is_empty() {
            return Ok(json!([]));
        }
        let aliases = self.aliases(account)?;
        let exact = aliases.get(&query).is_some();
        let fuzzy = std::env::var("TELEGRAM_CONTACT_FUZZY")
            .ok()
            .is_none_or(|value| {
                !["0", "false", "no", "off"].contains(&value.to_ascii_lowercase().as_str())
            });
        if !exact && (identifier_like(&query) || !fuzzy) {
            return Ok(json!([]));
        }
        let mut rows = Vec::new();
        for (alias, record) in aliases.as_object().expect("Alias store validated") {
            if exact && alias != &query || !exact && !covers(&query, alias) {
                continue;
            }
            let peer = &record["peer"];
            let id = alias_peer_id(peer)
                .or_else(|| record.get("id").cloned())
                .unwrap_or(Value::Null);
            rows.push(json!({"alias":alias,"peer":peer,"id":id,"name":record["name"],"account":record["account"],"needs_migration":record["needs_migration"],"favorite":true,"match":if alias==&query{"exact"}else{"similar"}}));
        }
        rows.sort_by(|left, right| {
            (left["match"] != "exact")
                .cmp(&(right["match"] != "exact"))
                .then_with(|| left["alias"].as_str().cmp(&right["alias"].as_str()))
        });
        rows.truncate(10);
        Ok(json!(rows))
    }

    pub fn alias_set(
        &self,
        account: &str,
        alias: &str,
        peer: Value,
        replace: bool,
    ) -> Result<Value> {
        self.migrate_aliases()?;
        let key = alias_key(alias);
        if key.is_empty() || key.chars().count() > 256 {
            bail!("An alias must contain 1-256 characters");
        }
        if identifier_like(&key) {
            bail!("Alias would shadow a real Telegram identifier; use a distinct nickname");
        }
        let label = normalize_key(&json!(account));
        if label.is_empty() {
            bail!("An alias requires an explicit account");
        }
        self.update_json("aliases-v2.json", json!({"accounts":{}}), |store| {
            if !store.is_object() || !store["accounts"].is_object() {
                bail!("Alias store is invalid; existing data was preserved");
            }
            let accounts = store["accounts"]
                .as_object_mut()
                .context("Alias accounts must be an object")?;
            let rows = accounts.entry(label.clone()).or_insert_with(|| json!({}));
            let rows = rows
                .as_object_mut()
                .context("Account alias store is invalid; existing data was preserved")?;
            let mut name = Value::Null;
            if let Some(previous) = rows.get(&key) {
                let previous_id = alias_peer_id(&previous["peer"]);
                let current_id = alias_peer_id(&peer);
                let same_id = previous_id.is_some() && previous_id == current_id;
                if same_id {
                    name = previous["name"].clone();
                }
                if previous["peer"] != peer && !same_id && !replace {
                    bail!(
                        "Alias already points at another contact; confirm then pass replace=true"
                    );
                }
            }
            rows.insert(
                key.clone(),
                json!({"peer":peer,"id":alias_peer_id(&peer),"account":label,"name":name}),
            );
            Ok(json!({"saved":true,"alias":key,"account":label,"resolved":peer}))
        })
    }

    pub fn alias_remove(&self, account: &str, alias: &str) -> Result<bool> {
        self.migrate_aliases()?;
        self.update_json("aliases-v2.json", json!({"accounts":{}}), |store| {
            let accounts = store
                .get_mut("accounts")
                .and_then(Value::as_object_mut)
                .context("Alias store is invalid; existing data was preserved")?;
            let Some(rows) = accounts.get_mut(&account.to_lowercase()) else {
                return Ok(false);
            };
            let rows = rows
                .as_object_mut()
                .context("Account alias store is invalid; existing data was preserved")?;
            Ok(rows.remove(&alias_key(alias)).is_some())
        })
    }
}

fn same_word(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left: Vec<_> = left.chars().collect();
    let right: Vec<_> = right.chars().collect();
    let shared = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    if left.len() - shared > 3 || right.len() - shared > 3 {
        return false;
    }
    if !(shared >= 4 || left.len() == right.len() && shared + 1 == left.len()) {
        return false;
    }
    matching_count(&left, &right) * 200 >= (left.len() + right.len()) * 65
}

fn matching_count(left: &[char], right: &[char]) -> usize {
    let (mut longest, mut a_start, mut b_start) = (0, 0, 0);
    let mut previous = vec![0; right.len() + 1];
    for (a, lc) in left.iter().enumerate() {
        let mut current = vec![0; right.len() + 1];
        for (b, rc) in right.iter().enumerate() {
            if lc == rc {
                current[b + 1] = previous[b] + 1;
                if current[b + 1] > longest {
                    longest = current[b + 1];
                    a_start = a + 1 - longest;
                    b_start = b + 1 - longest;
                }
            }
        }
        previous = current;
    }
    if longest == 0 {
        return 0;
    }
    longest
        + matching_count(&left[..a_start], &right[..b_start])
        + matching_count(&left[a_start + longest..], &right[b_start + longest..])
}

fn covers(query: &str, alias: &str) -> bool {
    let query: Vec<_> = query.split_whitespace().collect();
    let alias: Vec<_> = alias.split_whitespace().collect();
    if query.len() > alias.len() {
        return false;
    }
    let mut taken = vec![None; alias.len()];
    fn assign(
        token: usize,
        query: &[&str],
        alias: &[&str],
        taken: &mut [Option<usize>],
        seen: &mut [bool],
    ) -> bool {
        for (index, word) in alias.iter().enumerate() {
            if seen[index] || !same_word(query[token], word) {
                continue;
            }
            seen[index] = true;
            if taken[index].is_none_or(|other| assign(other, query, alias, taken, seen)) {
                taken[index] = Some(token);
                return true;
            }
        }
        false
    }
    (0..query.len()).all(|token| {
        assign(
            token,
            &query,
            &alias,
            &mut taken,
            &mut vec![false; alias.len()],
        )
    })
}
