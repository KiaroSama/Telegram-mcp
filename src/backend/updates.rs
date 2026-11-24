//! Metadata-only authenticated callbacks and conservative cloud update cursors.
use crate::{
    server::access::AccessPolicy,
    session::PersistentSession,
    state::{IncomingEvent, State},
    tools::local::upstream_events::{message_reply_id, message_topic_id},
};
use anyhow::{Context, Result};
#[cfg(test)]
use grammers_client::update::Update;
use grammers_client::{
    Client,
    peer::{Peer, User},
};
use grammers_mtsender::{
    InvocationError, SenderPoolFatHandle, UpdatesConfiguration, UpdatesReceiver,
};
use grammers_session::{
    Session,
    types::{PeerId, PeerInfo, PeerKind, UpdateState, UpdatesState},
    updates::{MessageBox, State as DeliveredState, UpdatesLike},
};
use grammers_tl_types as tl;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

struct MessageContext {
    chat_id: String,
    chat_type: &'static str,
    forum: bool,
    username: Option<String>,
    name: String,
    bot: bool,
    sender_id: Option<String>,
    sender_name: Option<String>,
    sender_bot: bool,
}

fn peer_name(peer: &Peer) -> String {
    let name = match peer {
        Peer::User(user) => user.full_name(),
        _ => peer.name().unwrap_or_default().to_owned(),
    };
    name.chars().take(512).collect()
}

fn identities(raw: &tl::enums::Message) -> Option<(PeerId, Option<PeerId>)> {
    let (peer, from, out) = match raw {
        tl::enums::Message::Message(m) => (&m.peer_id, m.from_id.as_ref(), m.out),
        tl::enums::Message::Service(m) => (&m.peer_id, m.from_id.as_ref(), m.out),
        _ => return None,
    };
    let peer = PeerId::from(peer.clone());
    let sender = from.cloned().map(PeerId::from).or_else(|| {
        (peer.kind() == PeerKind::User).then(|| if out { PeerId::self_user() } else { peer })
    });
    Some((peer, sender))
}

#[cfg(test)]
fn context(message: &grammers_client::update::Message) -> Result<MessageContext> {
    let inner: &grammers_client::message::Message = message;
    context_parts(&inner.raw, message.peer(), message.sender())
}

fn context_parts(
    raw: &tl::enums::Message,
    peer: Option<&Peer>,
    sender: Option<&Peer>,
) -> Result<MessageContext> {
    let (peer_id, sender_id) =
        identities(raw).context("Message update has no concrete chat identity")?;
    let chat_type = match peer_id.kind() {
        PeerKind::User => "private",
        PeerKind::Chat => "group",
        PeerKind::Channel => match peer {
            Some(Peer::Group(group)) if group.is_megagroup() => "supergroup",
            // Missing metadata must not turn a broadcast into a group or an
            // unknown forum into its General topic. The event log omits these.
            _ => "channel",
        },
    };
    let forum = matches!(peer,Some(Peer::Group(group))
        if matches!(&group.raw,tl::enums::Chat::Channel(channel) if channel.forum));
    Ok(MessageContext {
        chat_id: peer_id
            .bot_api_dialog_id()
            .context("Message update has no concrete chat identity")?
            .to_string(),
        chat_type,
        forum,
        username: peer.and_then(Peer::username).map(str::to_owned),
        name: peer.map(peer_name).unwrap_or_default(),
        bot: matches!(peer, Some(Peer::User(user)) if user.is_bot()),
        // An outgoing private message may have only InputPeerSelf's sentinel.
        // An unknown sender stays unknown rather than exposing that sentinel.
        sender_id: sender_id
            .and_then(|id| id.bot_api_dialog_id())
            .map(|id| id.to_string()),
        sender_name: sender.map(peer_name).filter(|name| !name.is_empty()),
        sender_bot: matches!(sender, Some(Peer::User(user)) if user.is_bot()),
    })
}

/// Extract only the fields needed by the upstream topic/reply helpers. Text,
/// quoted text, media handles, entities and access hashes never enter this JSON.
fn message_fields(raw: &tl::enums::Message) -> Option<Value> {
    let (id, out, mentioned, date, reply, service, topic_create) = match raw {
        tl::enums::Message::Message(message) => (
            message.id,
            message.out,
            message.mentioned,
            message.date,
            message.reply_to.as_ref(),
            false,
            false,
        ),
        tl::enums::Message::Service(message) => (
            message.id,
            message.out,
            message.mentioned,
            message.date,
            message.reply_to.as_ref(),
            true,
            matches!(&message.action, tl::enums::MessageAction::TopicCreate(_)),
        ),
        tl::enums::Message::Empty(_) => return None,
    };
    let reply = match reply {
        Some(tl::enums::MessageReplyHeader::Header(header)) => json!({
            "forum_topic":header.forum_topic,
            "reply_to_msg_id":header.reply_to_msg_id,
            "reply_to_top_id":header.reply_to_top_id,
            "reply_to_peer_id":header.reply_to_peer_id.clone()
                .map(grammers_session::types::PeerId::from)
                .and_then(|id|id.bot_api_dialog_id()).map(|id|id.to_string()),
        }),
        _ => Value::Null,
    };
    Some(json!({"id":id,"out":out,"mentioned":mentioned,"date":date,
        "reply_to":reply,"service":service,
        "action":if topic_create {json!({"_":"messageActionTopicCreate"})}else{Value::Null}}))
}

fn project(account: &str, fields: &Value, peer: &MessageContext) -> Value {
    let out = fields["out"].as_bool().unwrap_or(false);
    let addressed = if out {
        Some(false)
    } else if peer.chat_type == "private" || fields["mentioned"] == true {
        Some(true)
    } else if reply_candidate(fields, peer) {
        fields["reply_outgoing"].as_bool()
    } else {
        Some(false)
    };
    let date = fields["date"]
        .as_i64()
        .and_then(|stamp| chrono::DateTime::from_timestamp(stamp, 0))
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let mut record = json!({
        "account":account,"chat_id":peer.chat_id,"chat_type":peer.chat_type,
        "message_id":fields["id"],"sender_id":peer.sender_id,
        "sender_name":peer.sender_name,"sender_bot":peer.sender_bot,
        "out":out,"addressed":addressed,
        "topic":message_topic_id(fields,peer.forum),"reply_to":message_reply_id(fields),
        "service":fields["service"],"date":date,"_chat_username":peer.username,
    });
    if addressed.is_none() {
        record["addressed_unknown"] = json!(true);
    }
    record
}

fn reply_candidate(fields: &Value, peer: &MessageContext) -> bool {
    fields["out"] == false
        && fields["mentioned"] != true
        && matches!(peer.chat_type, "group" | "supergroup")
        && message_reply_id(fields).is_some_and(|id| (1..=i32::MAX as i64).contains(&id))
}

/// The callback may inspect a reply's outgoing flag, without acknowledging its
/// read state. Never even create/poll that request for a disallowed event, a
/// topic's structural parent, or a reply into another chat.
async fn address_reply<F, Fut>(
    policy: &AccessPolicy,
    fields: &mut Value,
    peer: &MessageContext,
    fetch: F,
) where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Option<bool>>>,
{
    let topic = message_topic_id(fields, peer.forum);
    if !policy.allows_message_event(&peer.chat_id, peer.username.as_deref(), topic)
        || !reply_candidate(fields, peer)
        || fields["reply_to"]["reply_to_peer_id"]
            .as_str()
            .is_some_and(|chat| chat != peer.chat_id)
    {
        return;
    }
    if let Ok(Ok(Some(outgoing))) =
        tokio::time::timeout(std::time::Duration::from_secs(5), fetch()).await
    {
        fields["reply_outgoing"] = json!(outgoing);
    }
}

fn publish(
    state: &Arc<State>,
    policy: &AccessPolicy,
    account: &str,
    fields: &Value,
    peer: &MessageContext,
) -> Result<()> {
    let record = project(account, fields, peer);
    let topic = record["topic"].as_i64();
    if !policy.allows_message_event(&peer.chat_id, peer.username.as_deref(), topic) {
        return Ok(());
    }
    state.record_message_event(record)?;
    if peer.chat_type == "private" && fields["out"] == false {
        state.record_incoming(IncomingEvent {
            account: account.to_owned(),
            chat_id: peer.chat_id.clone(),
            message_id: fields["id"].to_string(),
            name: peer.name.clone(),
            username: peer.username.clone(),
            bot: peer.bot,
        })?;
    }
    Ok(())
}

/// Difference siblings can share the final batch pts. This adapter is called
/// only after the complete batch was processed; seq/date remain container-wide
/// and unchanged. Secondary/qts is exclusively owned by its separate backend.
fn cloud_cursor(current: &UpdatesState, delivered: &DeliveredState) -> Option<UpdateState> {
    match delivered.message_box {
        Some(MessageBox::Common { pts }) if pts > 0 && pts > current.pts => {
            Some(UpdateState::Primary {
                pts,
                date: current.date,
                seq: current.seq,
            })
        }
        Some(MessageBox::Channel { channel_id, pts })
            if channel_id > 0
                && pts > 0
                && current
                    .channels
                    .iter()
                    .find(|channel| channel.id == channel_id)
                    .is_none_or(|channel| pts > channel.pts) =>
        {
            Some(UpdateState::Channel {
                id: channel_id,
                pts,
            })
        }
        _ => None,
    }
}

async fn checkpoint(session: &PersistentSession, delivered: &DeliveredState) -> Result<()> {
    let current = session.updates_state().await?;
    if let Some(update) = cloud_cursor(&current, delivered) {
        session.set_update_state(update).await?;
    }
    Ok(())
}

async fn process_batch<F, Fut>(
    session: &PersistentSession,
    batch: Vec<(tl::enums::Update, DeliveredState)>,
    mut deliver: F,
) -> Result<()>
where
    F: FnMut(tl::enums::Update) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    session.check_health()?;
    let mut delivered = Vec::new();
    for (update, position) in batch {
        session.check_health()?;
        if deliver(update).await? {
            delivered.push(position);
        }
    }
    for position in delivered {
        session.check_health()?;
        checkpoint(session, &position).await?;
    }
    Ok(())
}

fn retry_delay(error: &InvocationError, failures: u32) -> Option<Duration> {
    let delay = Duration::from_millis((250u64 << failures.min(7)).min(30_000));
    match error {
        InvocationError::Io(_) => Some(delay),
        InvocationError::Rpc(rpc) if rpc.code >= 500 => Some(delay),
        InvocationError::Rpc(rpc) if rpc.code == 420 && rpc.name == "FLOOD_WAIT" => rpc
            .value
            .filter(|seconds| *seconds <= 86_400)
            .map(|seconds| delay.max(Duration::from_secs(u64::from(seconds)))),
        InvocationError::Transport(grammers_mtproto::transport::Error::BadStatus {
            status: 429,
        }) => Some(delay),
        _ => None,
    }
}

type Batch = (
    Vec<(tl::enums::Update, DeliveredState)>,
    Vec<tl::enums::User>,
    Vec<tl::enums::Chat>,
);
trait BatchReader: Send {
    fn read(
        &mut self,
    ) -> impl std::future::Future<Output = std::result::Result<Batch, InvocationError>> + Send;
}
impl BatchReader for UpdatesReceiver {
    async fn read(&mut self) -> std::result::Result<Batch, InvocationError> {
        self.next().await
    }
}
async fn next_batch(reader: &mut impl BatchReader, session: &PersistentSession) -> Option<Batch> {
    let mut failures = 0u32;
    loop {
        if session.check_health().is_err() {
            return None;
        }
        let next = tokio::select! {biased;_=session.wait_for_write_uncertainty()=>return None,result=reader.read()=>result};
        match next {
            Ok(batch) => return Some(batch),
            Err(error) => {
                let Some(delay) = retry_delay(&error, failures) else {
                    tracing::warn!(
                        "Telegram update stream stopped; its account connection is retired"
                    );
                    return None;
                };
                failures = failures.saturating_add(1);
                tracing::warn!("Telegram update read failed temporarily; retry is scheduled");
                tokio::select! {biased;_=session.wait_for_write_uncertainty()=>return None,_=tokio::time::sleep(delay)=>{}}
            }
        }
    }
}

async fn deliver_message(
    client: &Client,
    session: &PersistentSession,
    state: &Arc<State>,
    policy: &AccessPolicy,
    account: &str,
    peers: &HashMap<PeerId, Peer>,
    update: tl::enums::Update,
) -> Result<bool> {
    let raw = match update {
        tl::enums::Update::NewMessage(message) => message.message,
        tl::enums::Update::NewChannelMessage(message) => message.message,
        _ => return Ok(false),
    };
    let Some((peer_id, sender_id)) = identities(&raw) else {
        return Ok(false);
    };
    let peer = context_parts(
        &raw,
        peers.get(&peer_id),
        sender_id.and_then(|id| peers.get(&id)),
    )?;
    let mut fields = message_fields(&raw).context("Missing message metadata")?;
    let reply_id = message_reply_id(&fields).and_then(|id| i32::try_from(id).ok());
    address_reply(policy, &mut fields, &peer, || async {
        let reference = if let Some(peer) = peers.get(&peer_id) {
            peer.to_ref()
                .await
                .map_err(|_| anyhow::anyhow!("Reply peer reference unavailable"))?
        } else {
            session.peer_ref(peer_id).await?
        };
        let Some(reference) = reference else {
            return Ok(None);
        };
        let Some(id) = reply_id else { return Ok(None) };
        let reply = client
            .get_messages_by_id(reference, &[id])
            .await?
            .into_iter()
            .flatten()
            .next();
        Ok(reply.and_then(|reply| {
            if reply.peer_id() != peer_id {
                return None;
            }
            let reply_fields = message_fields(&reply.raw)?;
            let topic = message_topic_id(&reply_fields, peer.forum);
            policy
                .allows_message_event(&peer.chat_id, peer.username.as_deref(), topic)
                .then(|| reply.outgoing())
        }))
    })
    .await;
    publish(state, policy, account, &fields, &peer)?;
    Ok(true)
}

#[cfg(test)]
async fn run(
    client: Client,
    handle: SenderPoolFatHandle,
    updates: tokio::sync::mpsc::Receiver<UpdatesLike>,
    state: Arc<State>,
    session: Arc<PersistentSession>,
    account: String,
) {
    run_with_secret(client, handle, updates, state, session, account, None).await;
}

pub(super) async fn publish_recovered_cloud(
    client: &Client,
    state: &Arc<State>,
    session: &Arc<PersistentSession>,
    account: &str,
    batches: &[kiaro_secret_chat::ingress::IngressBatch],
) -> Result<()> {
    if batches.is_empty() {
        return Ok(());
    }
    let policy = AccessPolicy::from_environment()?;
    for batch in batches {
        let (updates, users, chats) = batch
            .updates()
            .context("Cannot decode recovered ingress updates")?;
        let peers = users
            .into_iter()
            .map(|user| Peer::User(User::from_raw(client, user)))
            .chain(chats.into_iter().map(|chat| Peer::from_raw(client, chat)))
            .map(|peer| (peer.id(), peer))
            .collect::<HashMap<_, _>>();
        session
            .cache_peers(peers.values().map(PeerInfo::from).collect())
            .await?;
        for (update, _) in updates {
            let _ =
                deliver_message(client, session, state, &policy, account, &peers, update).await?;
        }
    }
    Ok(())
}

pub(super) async fn run_with_secret(
    client: Client,
    handle: SenderPoolFatHandle,
    updates: tokio::sync::mpsc::Receiver<UpdatesLike>,
    state: Arc<State>,
    session: Arc<PersistentSession>,
    account: String,
    secret: Option<Arc<crate::secret::SecretRuntime>>,
) {
    // The pinned sender reports home-DC disconnects to this receiver, which
    // requests a difference on the same pool and persisted account cursor.
    let create = UpdatesReceiver::create(
        handle.thin.clone(),
        handle.session.clone(),
        updates,
        UpdatesConfiguration { catch_up: true },
    );
    let started = tokio::select! {
        biased;
        _=session.wait_for_write_uncertainty()=>{handle.quit();return;},
        result=create=>result,
    };
    let Ok(mut stream) = started else {
        tracing::warn!("Telegram message update stream could not start");
        handle.quit();
        return;
    };
    loop {
        let Some((batch, users, chats)) = next_batch(&mut stream, &session).await else {
            break;
        };
        let outcome = async {
            session.check_health()?;
            let peers = users
                .into_iter()
                .map(|user| Peer::User(User::from_raw(&client, user)))
                .chain(chats.into_iter().map(|chat| Peer::from_raw(&client, chat)))
                .map(|peer| (peer.id(), peer))
                .collect::<HashMap<_, _>>();
            session
                .cache_peers(peers.values().map(PeerInfo::from).collect())
                .await?;
            let policy = AccessPolicy::from_environment()?;
            process_batch(&session, batch, |raw| {
                deliver_message(&client, &session, &state, &policy, &account, &peers, raw)
            })
            .await?;
            if let Some(secret) = &secret
                && let Err(error) = secret.manager().drain_ingress().await
            {
                use kiaro_secret_chat::error::Error;
                let cause = match &error {
                    Error::Context(context) => context.refusal(),
                    error => error,
                };
                if matches!(
                    cause,
                    Error::ManagerStopping
                        | Error::NotRunning
                        | Error::Authorization
                        | Error::InvalidState
                ) {
                    return Err(error.into());
                }
                tracing::warn!("Secret-chat ingress replay deferred; cloud updates remain active");
            }
            Ok::<_, anyhow::Error>(())
        };
        let outcome = tokio::select! {biased;_=session.wait_for_write_uncertainty()=>break,result=outcome=>result};
        if outcome.is_err() {
            tracing::warn!(
                "Telegram message metadata could not be recorded; update consumer stopped"
            );
            break;
        }
    }
    handle.quit();
}

#[cfg(test)]
#[path = "../../tests/unit/backend_updates.rs"]
mod tests;
