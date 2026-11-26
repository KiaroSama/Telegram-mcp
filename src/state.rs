//! Private local state and file access boundaries, independent of Telegram.

mod aliases;
mod events;
mod grants;
mod paths;
pub(crate) mod storage;
mod stream;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub use aliases::{alias_key, alias_peer_id, identifier_like};
pub use events::{EventJournal, IncomingEvent};
pub(crate) use grants::insert_approval;
pub use paths::{
    DANGEROUS_SUFFIXES, FileAccess, safe_suffix, sanitize_download_filename,
    sanitize_download_path, validate_path_syntax,
};
pub(crate) use paths::{ScopedPathGrant, canonical_existing, with_temporary_grants};
pub use stream::{EventLog, EventQuery};

pub struct State {
    #[cfg(test)]
    fail_directory_sync: AtomicBool,
    root: PathBuf,
    roots: Vec<PathBuf>,
    installation: PathBuf,
    env_roots_enabled: bool,
    legacy_alias_source: Option<PathBuf>,
    live_alias_file: Option<PathBuf>,
    feed_file: Option<PathBuf>,
    feed_autostart: AtomicBool,
    pub(crate) events: Mutex<EventJournal>,
    pub(crate) changed: tokio::sync::Notify,
    pub(crate) message_events: Mutex<EventLog>,
}

impl State {
    pub fn from_environment() -> Result<Self> {
        let root = if let Some(path) = std::env::var_os("TELEGRAM_STATE_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("telegram-mcp")
        } else {
            PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .context("HOME or USERPROFILE is not set")?,
            )
            .join(".local/state/telegram-mcp")
        };
        let installation =
            std::env::current_dir().context("Cannot resolve installation directory")?;
        let roots = match std::env::var("TELEGRAM_FILE_ROOTS") {
            Ok(text) if !text.trim().is_empty() => paths::parse_roots_setting(&text)?,
            _ => {
                let files = installation.join("files");
                storage::private_directory(&files)?;
                storage::private_directory(&files.join("outbox"))?;
                vec![files]
            }
        };
        let mut state = Self::at_with_installation(root, roots, installation)?;
        state.env_roots_enabled = true;
        let alias_env = std::env::var_os("TELEGRAM_ALIASES_FILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        state.legacy_alias_source = alias_env.clone();
        state.live_alias_file = alias_env;
        state.feed_file = std::env::var_os("TELEGRAM_EVENT_FEED_FILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        state.feed_autostart.store(
            std::env::var("TELEGRAM_EVENT_FEED")
                .ok()
                .is_some_and(|value| {
                    ["1", "true", "yes", "on"].contains(&value.to_ascii_lowercase().as_str())
                }),
            Ordering::Relaxed,
        );
        if let Ok(raw) = std::env::var("TELEGRAM_EVENT_LOG_SIZE") {
            let capacity = raw.parse::<usize>().unwrap_or(2000);
            state.message_events = Mutex::new(EventLog::new(capacity));
        }
        Ok(state)
    }

    pub fn at(root: PathBuf, roots: Vec<PathBuf>) -> Result<Self> {
        Self::at_with_installation(root, roots, std::env::current_dir()?)
    }

    pub fn at_with_installation(
        root: PathBuf,
        roots: Vec<PathBuf>,
        installation: PathBuf,
    ) -> Result<Self> {
        storage::private_directory(&root)?;
        let root = root
            .canonicalize()
            .context("Cannot resolve private state directory")?;
        let installation = installation
            .canonicalize()
            .context("Cannot resolve installation directory")?;
        let mut normalized = Vec::new();
        for path in roots {
            let path = path
                .canonicalize()
                .context("An allowed file root does not exist")?;
            if !path.is_dir() {
                bail!("An allowed file root is not a directory");
            }
            if !normalized.contains(&path) {
                normalized.push(path);
            }
        }
        Ok(Self {
            #[cfg(test)]
            fail_directory_sync: AtomicBool::new(false),
            root,
            roots: normalized,
            installation,
            env_roots_enabled: false,
            legacy_alias_source: None,
            live_alias_file: None,
            feed_file: None,
            feed_autostart: AtomicBool::new(false),
            events: Mutex::new(EventJournal::default()),
            changed: tokio::sync::Notify::new(),
            message_events: Mutex::new(EventLog::default()),
        })
    }

    pub fn global() -> Result<Arc<Self>> {
        static GLOBAL: OnceLock<Arc<State>> = OnceLock::new();
        if let Some(state) = GLOBAL.get() {
            return Ok(Arc::clone(state));
        }
        let state = Arc::new(Self::from_environment()?);
        let _ = GLOBAL.set(state);
        Ok(Arc::clone(GLOBAL.get().expect("State initialized")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    #[cfg(all(test, unix))]
    pub(crate) fn fail_next_directory_sync_for_test(&self) {
        self.fail_directory_sync
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(crate) fn take_directory_sync_failure_for_test(&self) -> bool {
        self.fail_directory_sync
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }
    pub fn file_roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn default_downloads(&self) -> Result<PathBuf> {
        let path = self
            .roots
            .first()
            .context("No allowed file roots are configured")?
            .join("downloads");
        self.resolve_file(&path, FileAccess::Write)
    }

    pub fn ghost_mode(&self, account: Option<&str>, chat: Option<&Value>) -> Value {
        let keys: Vec<String> = chat.into_iter().map(normalize_key).collect();
        self.ghost_mode_for_keys(account, &keys)
    }

    pub fn ghost_mode_for_keys(&self, account: Option<&str>, chat_keys: &[String]) -> Value {
        let defaults = json!({"default":true,"accounts":{},"chats":{}});
        let (data, error) = match self.read_json("ghost.json", defaults.clone()) {
            Ok(data) if data.is_object() => (data, None),
            _ => (
                defaults,
                Some("ghost.json unreadable; treated as ghost mode ON"),
            ),
        };
        let account_value = account.and_then(|a| data["accounts"].get(a.to_lowercase()));
        let chat_value = account.and_then(|a| {
            let scoped = data["chats"].get(a.to_lowercase())?;
            chat_keys
                .iter()
                .filter(|k| !k.is_empty())
                .find_map(|key| scoped.get(key))
        });
        let effective = if error.is_some() {
            true
        } else {
            chat_value
                .and_then(Value::as_bool)
                .or_else(|| account_value.and_then(Value::as_bool))
                .or_else(|| data["default"].as_bool())
                .unwrap_or(true)
        };
        json!({"default":data["default"],"account":account_value,"chat":chat_value,"effective":effective,
            "error":error,"suppresses":["read markers","online presence after server activity","story views","voice listened marks","typing indicators","channel view counts"]})
    }

    pub fn set_ghost_mode(
        &self,
        enabled: bool,
        account: Option<&str>,
        chat: Option<&Value>,
    ) -> Result<Value> {
        let keys: Vec<String> = chat.into_iter().map(normalize_key).collect();
        self.set_ghost_mode_for_keys(enabled, account, &keys)
    }

    pub fn set_ghost_mode_for_keys(
        &self,
        enabled: bool,
        account: Option<&str>,
        chat_keys: &[String],
    ) -> Result<Value> {
        let normalized_keys: Vec<String> = chat_keys
            .iter()
            .map(|k| k.trim().trim_start_matches('@').to_lowercase())
            .filter(|k| !k.is_empty())
            .collect();
        if !normalized_keys.is_empty() && account.is_none() {
            bail!("A chat setting requires its account");
        }
        self.update_json(
            "ghost.json",
            json!({"default":true,"accounts":{},"chats":{}}),
            |data| {
                if !data.is_object() {
                    bail!("ghost.json is not an object; nothing was overwritten");
                }
                if ["accounts", "chats"]
                    .iter()
                    .any(|field| !data[*field].is_null() && !data[*field].is_object())
                    || data["chats"].as_object().is_some_and(|accounts| {
                        accounts
                            .values()
                            .any(|chats| !chats.is_null() && !chats.is_object())
                    })
                {
                    bail!("ghost.json nested settings are invalid; nothing was overwritten");
                }
                if let Some(a) = account
                    && !normalized_keys.is_empty()
                {
                    if data["chats"].is_null() {
                        data["chats"] = json!({});
                    }
                    if data["chats"][a.to_lowercase()].is_null() {
                        data["chats"][a.to_lowercase()] = json!({});
                    }
                    for key in &normalized_keys {
                        data["chats"][a.to_lowercase()][key] = json!(enabled);
                    }
                } else if let Some(a) = account {
                    if data["accounts"].is_null() {
                        data["accounts"] = json!({});
                    }
                    data["accounts"][a.to_lowercase()] = json!(enabled);
                } else {
                    data["default"] = json!(enabled);
                }
                Ok(())
            },
        )?;
        Ok(self.ghost_mode_for_keys(account, &normalized_keys))
    }

    pub fn with_feed_file(mut self, path: Option<PathBuf>) -> Self {
        self.feed_file = path;
        self
    }

    pub fn with_live_alias_file(mut self, path: Option<PathBuf>) -> Self {
        self.live_alias_file = path;
        self
    }
}

pub fn normalize_key(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
        .trim()
        .trim_start_matches('@')
        .to_lowercase()
}
