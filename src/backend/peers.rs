use super::*;
use grammers_session::types::{ChannelKind, PeerAuth, PeerId, PeerInfo, PeerKind};

fn peer_integer(peer: &Value, field: &str) -> Option<i64> {
    peer[field]
        .as_str()
        .and_then(|text| text.parse().ok())
        .or_else(|| peer[field].as_i64())
}

/// RPCs may return peers directly, as a Vector<User>, or in the usual wrapper
/// arrays. Only those explicit positions are peer records; arbitrary nested
/// message content is never treated as authoritative cache data.
fn peer_infos(value: &Value) -> Vec<PeerInfo> {
    std::iter::once(value)
        .chain(value.as_array().into_iter().flatten())
        .chain(
            value
                .get("users")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .chain(
            value
                .get("chats")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|peer| {
            let id = peer_integer(peer, "id").filter(|id| *id > 0)?;
            // A `min` access hash is scoped to its source message, not full
            // peer authority. None also preserves previously cached full auth
            // through Session::cache_peers / PeerInfo::extend_info.
            let auth = if peer["min"] == true {
                None
            } else {
                peer_integer(peer, "access_hash").map(PeerAuth::from_hash)
            };
            Some(match peer["_"].as_str()? {
                "user" => PeerInfo::User {
                    id,
                    auth,
                    bot: peer["bot"].as_bool(),
                    is_self: peer["self"].as_bool(),
                },
                "userEmpty" => PeerInfo::User {
                    id,
                    auth: None,
                    bot: None,
                    is_self: None,
                },
                "chat" | "chatForbidden" | "chatEmpty" => PeerInfo::Chat { id },
                "channel" | "channelForbidden" => {
                    let kind = if peer["gigagroup"] == true {
                        Some(ChannelKind::Gigagroup)
                    } else if peer["broadcast"] == true {
                        Some(ChannelKind::Broadcast)
                    } else if peer["megagroup"] == true {
                        Some(ChannelKind::Megagroup)
                    } else {
                        None
                    };
                    PeerInfo::Channel { id, auth, kind }
                }
                "community" | "communityForbidden" => PeerInfo::Channel {
                    id,
                    auth,
                    kind: Some(ChannelKind::Community),
                },
                _ => return None,
            })
        })
        .collect()
}

impl Native {
    pub(super) async fn resolve(&self, input: &Value) -> Result<Value> {
        if input.is_object() {
            let encoded = Schema::global().encode_object(input)?;
            Schema::global().decode("InputPeer", &encoded)?;
            return Ok(input.clone());
        }
        let text = match input {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => bail!("Peer must be a numeric id, username or saved alias"),
        };
        let text = text.trim();
        if matches!(
            text.to_ascii_lowercase().as_str(),
            "me" | "self" | "saved" | "saved messages"
        ) {
            return Ok(json!({"_":"inputPeerSelf"}));
        }
        if let Some(alias) = self.0.state.alias_lookup(&self.0.label, text)? {
            let target = &alias["peer"];
            if target.is_number() || target.as_str().is_some_and(|s| s.parse::<i64>().is_ok()) {
                return Box::pin(self.resolve(target)).await;
            }
            let bytes = Schema::global().encode_object(target)?;
            return Schema::global().decode("InputPeer", &bytes);
        }
        if let Ok(id) = text.parse::<i64>() {
            let peer_id = PeerId::from_bot_api_dialog_id(id).context("Invalid Telegram peer id")?;
            if let Some(reference) = self.0.session.peer_ref(peer_id).await? {
                let raw: tl::enums::InputPeer = reference.into();
                return Schema::global().decode("InputPeer", &raw.to_bytes());
            }
            if matches!(peer_id.kind(), PeerKind::Chat) {
                return Ok(
                    json!({"_":"inputPeerChat","chat_id":peer_id.bare_id_unchecked().to_string()}),
                );
            }
            // Warm dialogs once before declaring a missing access hash. Never guess
            // access_hash=0 for a user/channel, which can target the wrong entity.
            let mut dialogs = self.0.client.iter_dialogs();
            for _ in 0..1000 {
                let Some(dialog) = dialogs.next().await? else {
                    break;
                };
                if dialog.peer().id() == peer_id {
                    let reference = dialog
                        .peer()
                        .to_ref()
                        .await
                        .map_err(|_| anyhow::anyhow!("Peer reference storage failed"))?
                        .context("Peer has no usable access reference")?;
                    let raw: tl::enums::InputPeer = reference.into();
                    return Schema::global().decode("InputPeer", &raw.to_bytes());
                }
            }
            bail!(
                "Peer access reference was not found in the bounded dialog search; use its username"
            );
        }
        let username = text.trim_start_matches('@');
        if !(1..=64).contains(&username.len())
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            bail!("Invalid Telegram username or unknown saved alias");
        }
        let peer = self
            .0
            .client
            .resolve_username(username)
            .await?
            .context("Telegram username was not found")?;
        let reference = peer
            .to_ref()
            .await
            .map_err(|_| anyhow::anyhow!("Peer reference storage failed"))?
            .context("Peer has no access reference")?;
        let raw: tl::enums::InputPeer = reference.into();
        Schema::global().decode("InputPeer", &raw.to_bytes())
    }
}

pub(super) async fn cache_rpc_outcome(session: &PersistentSession, result: Value) -> Result<Value> {
    // Telegram's response already establishes the remote outcome. Peer caching
    // is supplementary local work and cannot turn an accepted mutation into a
    // retryable failure. PersistentSession leaves failed snapshots unpublished.
    if session.cache_peers(peer_infos(&result)).await.is_err() {
        tracing::warn!("Telegram completed an RPC but supplementary peer caching failed");
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../../tests/unit/backend_peers.rs"]
mod tests;
