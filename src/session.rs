//! Durable grammers Session implementation. Every key/update write is persisted
//! before acknowledging success; an exclusive lease prevents two sender pools
//! from reusing the same MTProto authorization key concurrently.
mod custody;
mod import;
mod transaction;
use crate::state::State;
use anyhow::{Context, Result, bail, ensure};
pub use custody::{check_deferred_custody, check_deferred_custody_with_config};
use fs2::FileExt;
use grammers_session::{BoxFuture, Session, SessionData, types::*};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    home_dc: i32,
    dc_options: Vec<DcOption>,
    peer_infos: Vec<PeerInfo>,
    updates_state: UpdatesState,
    #[serde(default)]
    authorization_source: Option<String>,
    #[serde(default = "legacy_origin")]
    authorization_origin: String,
}
fn legacy_origin() -> String {
    "legacy".into()
}
#[derive(Debug)]
pub struct SessionError;
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Private session storage failed; inspect the saved snapshot before retrying"
        )
    }
}
impl std::error::Error for SessionError {}
pub struct PersistentSession {
    data: Mutex<SessionData>,
    state: Arc<State>,
    name: String,
    _lease: File,
    auth_leases: Mutex<std::collections::HashMap<String, File>>,
    binding: Mutex<Option<String>>,
    origin: Mutex<String>,
    write_uncertain: AtomicBool,
    health_changed: tokio::sync::Notify,
}
pub fn validate_label(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("Account label must contain 1-64 ASCII letters, digits, underscores or hyphens");
    }
    Ok(())
}
impl PersistentSession {
    pub(crate) fn label(&self) -> &str {
        &self.name
    }

    pub fn open(state: Arc<State>, name: &str) -> Result<Self> {
        validate_label(name)?;
        let path = state.root().join(format!("{name}.session.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        if path
            .symlink_metadata()
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            bail!("Invalid session lease file");
        }
        let lease = options
            .open(&path)
            .context("Cannot acquire private session lease")?;
        FileExt::try_lock_exclusive(&lease)
            .context("Account is already open; connect through its shared service")?;
        let value = state.read_json(&format!("{name}.session.json"), serde_json::Value::Null)?;
        let (data, binding, origin) = if value.is_null() {
            (SessionData::default(), None, "native".into())
        } else {
            let snapshot: Snapshot = serde_json::from_value(value)
                .context("Invalid session; preserved without resetting authorization")?;
            (
                SessionData {
                    home_dc: snapshot.home_dc,
                    dc_options: snapshot.dc_options.into_iter().map(|d| (d.id, d)).collect(),
                    peer_infos: snapshot
                        .peer_infos
                        .into_iter()
                        .map(|p| (p.id(), p))
                        .collect(),
                    updates_state: snapshot.updates_state,
                },
                snapshot.authorization_source,
                snapshot.authorization_origin,
            )
        };
        let session = Self {
            data: Mutex::new(data),
            state,
            name: name.to_owned(),
            _lease: lease,
            auth_leases: Mutex::new(std::collections::HashMap::new()),
            binding: Mutex::new(binding),
            origin: Mutex::new(origin),
            write_uncertain: AtomicBool::new(false),
            health_changed: tokio::sync::Notify::new(),
        };
        session.acquire_auth_leases(&*session.data()?)?;
        Ok(session)
    }
    fn data(&self) -> std::result::Result<MutexGuard<'_, SessionData>, SessionError> {
        self.check_health()?;
        let data = self.data.lock().map_err(|_| SessionError)?;
        self.check_health()?;
        Ok(data)
    }
    pub(crate) fn check_health(&self) -> std::result::Result<(), SessionError> {
        if self.write_uncertain.load(Ordering::Acquire) {
            Err(SessionError)
        } else {
            Ok(())
        }
    }
    pub(crate) async fn wait_for_write_uncertainty(&self) {
        loop {
            let changed = self.health_changed.notified();
            tokio::pin!(changed);
            // Register before checking the flag so a commit between the check
            // and awaiting cannot lose notify_waiters' notification.
            changed.as_mut().enable();
            if self.check_health().is_err() {
                return;
            }
            changed.await;
        }
    }
    fn persist(&self, data: &SessionData) -> std::result::Result<(), SessionError> {
        self.check_health()?;
        self.acquire_auth_leases(data).map_err(|_| SessionError)?;
        let snapshot = Snapshot {
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
            peer_infos: data.peer_infos.values().cloned().collect(),
            updates_state: data.updates_state.clone(),
            authorization_source: self.binding.lock().map_err(|_| SessionError)?.clone(),
            authorization_origin: self.origin.lock().map_err(|_| SessionError)?.clone(),
        };
        let value = serde_json::to_value(snapshot).map_err(|_| SessionError)?;
        self.state
            .write_json(&format!("{}.session.json", self.name), &value)
            .map_err(|error| {
                if error
                    .downcast_ref::<crate::state::storage::DurabilityUncertain>()
                    .is_some()
                {
                    self.write_uncertain.store(true, Ordering::Release);
                    self.health_changed.notify_waiters();
                }
                SessionError
            })
    }
    /// Import authorization from a legacy v1 StringSession.
    pub fn import_string_session(&self, text: &str) -> Result<()> {
        use base64::Engine;
        if !text.starts_with('1') {
            bail!("Unsupported StringSession version");
        }
        let bytes = base64::engine::general_purpose::URL_SAFE
            .decode(&text[1..])
            .context("Invalid StringSession encoding")?;
        let ip_len = match bytes.len() {
            263 => 4,
            275 => 16,
            _ => bail!("Invalid StringSession length"),
        };
        let dc = bytes[0] as i32;
        let port = u16::from_be_bytes(bytes[1 + ip_len..3 + ip_len].try_into()?);
        ensure!(port != 0, "StringSession datacenter port must not be zero");
        ensure!(
            bytes[3 + ip_len..].iter().any(|byte| *byte != 0),
            "StringSession authorization key must not be all zero"
        );
        let mut current = self.data().map_err(anyhow::Error::new)?;
        let mut data = Self::clone_data(&current);
        if data.dc_options.values().any(|d| d.auth_key.is_some()) {
            bail!("Refusing to overwrite an existing authorization");
        }
        let option = data
            .dc_options
            .get_mut(&dc)
            .context("Unsupported StringSession datacenter")?;
        if ip_len == 4 {
            let ip = std::net::Ipv4Addr::new(bytes[1], bytes[2], bytes[3], bytes[4]);
            option.ipv4 = std::net::SocketAddrV4::new(ip, port);
        } else {
            let ip = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[1..17])?);
            option.ipv6 = std::net::SocketAddrV6::new(ip, port, 0, 0);
        }
        option.auth_key = Some(bytes[3 + ip_len..].try_into()?);
        data.home_dc = dc;
        self.persist_import(&data)?;
        *current = data;
        Ok(())
    }
}
impl Session for PersistentSession {
    type Error = SessionError;
    fn home_dc_id(&self) -> std::result::Result<i32, SessionError> {
        Ok(self.data()?.home_dc)
    }
    fn set_home_dc_id(&self, id: i32) -> BoxFuture<'_, std::result::Result<(), SessionError>> {
        Box::pin(async move { self.transaction(|d| d.home_dc = id) })
    }
    fn dc_option(&self, id: i32) -> std::result::Result<Option<DcOption>, SessionError> {
        Ok(self.data()?.dc_options.get(&id).cloned())
    }
    fn set_dc_option(
        &self,
        option: &DcOption,
    ) -> BoxFuture<'_, std::result::Result<(), SessionError>> {
        let option = option.clone();
        Box::pin(async move {
            self.transaction(|d| {
                d.dc_options.insert(option.id, option);
            })
        })
    }
    fn peer(
        &self,
        id: PeerId,
    ) -> BoxFuture<'_, std::result::Result<Option<PeerInfo>, SessionError>> {
        Box::pin(async move {
            let data = self.data()?;
            Ok(if id == PeerId::self_user() {
                data.peer_infos
                    .values()
                    .find(|peer| {
                        matches!(
                            peer,
                            PeerInfo::User {
                                is_self: Some(true),
                                ..
                            }
                        )
                    })
                    .cloned()
            } else {
                data.peer_infos.get(&id).cloned()
            })
        })
    }
    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, std::result::Result<(), SessionError>> {
        Box::pin(async move {
            self.transaction(|d| {
                d.peer_infos
                    .entry(peer.id())
                    .or_insert_with(|| peer.clone())
                    .extend_info(&peer);
            })
        })
    }
    fn cache_peers(
        &self,
        peers: Vec<PeerInfo>,
    ) -> BoxFuture<'_, std::result::Result<(), SessionError>> {
        Box::pin(async move {
            if peers.is_empty() {
                return self.check_health();
            }
            self.transaction(|d| {
                for peer in peers {
                    d.peer_infos
                        .entry(peer.id())
                        .or_insert_with(|| peer.clone())
                        .extend_info(&peer);
                }
            })
        })
    }
    fn updates_state(&self) -> BoxFuture<'_, std::result::Result<UpdatesState, SessionError>> {
        Box::pin(async move { Ok(self.data()?.updates_state.clone()) })
    }
    fn set_update_state(
        &self,
        update: UpdateState,
    ) -> BoxFuture<'_, std::result::Result<(), SessionError>> {
        Box::pin(async move {
            self.transaction(|d| match update {
                UpdateState::All(state) => d.updates_state = state,
                UpdateState::Primary { pts, date, seq } => {
                    d.updates_state.pts = pts;
                    d.updates_state.date = date;
                    d.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => d.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    d.updates_state.channels.retain(|c| c.id != id);
                    d.updates_state.channels.push(ChannelState { id, pts });
                }
            })
        })
    }
}
