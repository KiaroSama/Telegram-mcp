//! Metadata-only authenticated callbacks and conservative cloud update cursors.
use crate::{
    server::access::AccessPolicy,
    session::PersistentSession,
    state::{IncomingEvent, State},
    tools::local::upstream_events::{message_reply_id, message_topic_id},
};
use anyhow::{Context, Result};
use grammers_client::{Client, peer::Peer, update::Update};
use grammers_mtsender::UpdatesConfiguration;
use grammers_session::{
    Session,
    types::{PeerKind, UpdateState, UpdatesState},
    updates::{MessageBox, State as DeliveredState, UpdatesLike},
};
use grammers_tl_types as tl;
use serde_json::{Value, json};
use std::sync::Arc;

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

fn context(message: &grammers_client::update::Message) -> Result<MessageContext> {
    let peer = message.peer();
    let chat_type = match message.peer_id().kind() {
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
    let sender = message.sender();
    Ok(MessageContext {
        chat_id: message
            .peer_id()
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
        sender_id: message
            .sender_id()
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

/// Grammers supplies a per-event pts but container-wide seq/date. Keep the
/// latter unchanged: advancing them here could skip unconsumed siblings after
/// a crash. Secondary/qts state is exclusively owned by its separate backend.
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

pub(super) async fn run(
    client: Client,
    updates: tokio::sync::mpsc::Receiver<UpdatesLike>,
    state: Arc<State>,
    session: Arc<PersistentSession>,
    account: String,
) {
    let Ok(mut stream) = client
        .stream_updates(updates, UpdatesConfiguration { catch_up: true })
        .await
    else {
        tracing::warn!("Telegram message update stream could not start");
        return;
    };
    loop {
        let update = match stream.next().await {
            Ok(update) => update,
            Err(_) => {
                tracing::warn!("Telegram update stream stopped; reconnect is required");
                break;
            }
        };
        // Upstream's message metadata stream represents newly delivered
        // messages. Edits/deletions have no compatible event-kind contract.
        let Update::NewMessage(message) = &update else {
            continue;
        };
        let outcome = async {
            let policy = AccessPolicy::from_environment()?;
            let peer = context(message)?;
            let inner: &grammers_client::message::Message = message;
            let mut fields = message_fields(&inner.raw).context("Missing message metadata")?;
            address_reply(&policy, &mut fields, &peer, || async {
                // Official grammers get_reply_to_message invokes only
                // messages.getMessages/channels.getMessages, never readHistory.
                let reply = message.get_reply().await?;
                Ok(reply.and_then(|reply| {
                    if reply.peer_id() != message.peer_id() {
                        return None;
                    }
                    let reply_fields = message_fields(&reply.raw)?;
                    let reply_topic = message_topic_id(&reply_fields, peer.forum);
                    policy
                        .allows_message_event(&peer.chat_id, peer.username.as_deref(), reply_topic)
                        .then(|| reply.outgoing())
                }))
            })
            .await;
            publish(&state, &policy, &account, &fields, &peer)
        }
        .await;
        if outcome.is_err() {
            tracing::warn!(
                "Telegram message metadata could not be recorded; update consumer stopped"
            );
            break;
        }
        if checkpoint(&session, update.state()).await.is_err() {
            tracing::warn!(
                "Telegram message cursor could not be persisted; update consumer stopped"
            );
            break;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/backend_updates.rs"]
mod tests;
