use anyhow::{Result, ensure};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChannel {
    #[serde(rename = "dialog")]
    DialogChannel,
    #[serde(rename = "telegram_bot")]
    BotChannel,
    #[serde(rename = "saved_messages")]
    SavedMessagesChannel,
    OwnerTerminal,
}

impl ApprovalChannel {
    pub fn uses_message(self) -> bool {
        matches!(self, Self::BotChannel | Self::SavedMessagesChannel)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAvailability {
    Available,
    Unavailable,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelCapabilities {
    pub dialog: ChannelAvailability,
    pub bot: ChannelAvailability,
    pub saved_messages: ChannelAvailability,
    pub owner_terminal: ChannelAvailability,
}

impl ChannelCapabilities {
    /// Availability must be observed through trusted context for the acting account.
    pub fn availability(self, channel: ApprovalChannel) -> ChannelAvailability {
        match channel {
            ApprovalChannel::DialogChannel => self.dialog,
            ApprovalChannel::BotChannel => self.bot,
            ApprovalChannel::SavedMessagesChannel => self.saved_messages,
            ApprovalChannel::OwnerTerminal => self.owner_terminal,
        }
    }
}

pub fn dialog_available(trusted_human: bool, form_elicitation: bool, protocol: &str) -> bool {
    let valid_version = protocol.len() == 10
        && NaiveDate::parse_from_str(protocol, "%Y-%m-%d").is_ok_and(|d| d.to_string() == protocol);
    trusted_human && form_elicitation && valid_version && protocol < "2026-07-28"
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelOutcome {
    Unsupported,
    ApprovedOnce,
    ApprovedAlways,
    Denied,
    Cancelled,
    TimedOut,
    AmbiguousDelivery,
}

impl ChannelOutcome {
    pub fn may_fall_through(self) -> bool {
        self == Self::Unsupported
    }

    pub fn is_owner_reply(self) -> bool {
        matches!(
            self,
            Self::ApprovedOnce | Self::ApprovedAlways | Self::Denied | Self::Cancelled
        )
    }
}

/// The caller supplies these facts from the verified transport, never tool arguments.
#[derive(Clone, Copy)]
pub struct ReplyBinding<'a> {
    pub request_id: &'a str,
    pub owner: i64,
    pub channel: ApprovalChannel,
    pub account: &'a str,
    pub self_peer: Option<i64>,
    pub message_id: Option<i32>,
    pub code: &'a str,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRequest {
    request_id: String,
    owner: i64,
    channel: ApprovalChannel,
    account: String,
    self_peer: Option<i64>,
    message_ids: BTreeSet<i32>,
    code: String,
    expires_at_unix: u64,
    #[serde(default)]
    closed: bool,
}

pub(super) fn valid_code(code: &str) -> bool {
    (4..=54).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl PendingRequest {
    /// Telegram routes supply their verified private peer, and bind an observed message later.
    pub fn new(binding: ReplyBinding<'_>, expires_at_unix: u64) -> Result<Self> {
        ensure!(
            binding.message_id.is_none(),
            "Bind approval messages only after delivery"
        );
        let pending = Self {
            request_id: binding.request_id.to_owned(),
            owner: binding.owner,
            channel: binding.channel,
            account: binding.account.to_ascii_lowercase(),
            self_peer: binding.self_peer,
            message_ids: BTreeSet::new(),
            code: binding.code.to_ascii_uppercase(),
            expires_at_unix,
            closed: false,
        };
        ensure!(pending.valid(), "Invalid approval request binding");
        Ok(pending)
    }

    pub(super) fn valid(&self) -> bool {
        (1..=128).contains(&self.request_id.len())
            && self
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            && self.owner > 0
            && crate::session::validate_label(&self.account).is_ok()
            && self.expires_at_unix > 0
            && valid_code(&self.code)
            && if self.channel.uses_message() {
                self.self_peer.is_some_and(|id| id == self.owner)
                    && self.message_ids.iter().all(|&id| id > 0)
            } else {
                self.self_peer.is_none() && self.message_ids.is_empty()
            }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn account(&self) -> &str {
        &self.account
    }
    pub fn channel(&self) -> ApprovalChannel {
        self.channel
    }
    pub fn self_peer(&self) -> Option<i64> {
        self.self_peer
    }
    pub fn code(&self) -> &str {
        &self.code
    }

    pub(super) fn unbound(&self) -> bool {
        self.channel.uses_message() && self.message_ids.is_empty()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(super) fn message_ids(&self) -> &BTreeSet<i32> {
        &self.message_ids
    }

    pub(super) fn close(&mut self) {
        self.closed = true;
    }

    pub(super) fn bind_message(&mut self, delivered: ReplyBinding<'_>, now: u64) -> Result<i32> {
        ensure!(
            self.channel.uses_message() && self.matches_context(delivered, now),
            "Approval message binding does not match"
        );
        let id = delivered
            .message_id
            .filter(|&id| id > 0)
            .ok_or_else(|| anyhow::anyhow!("Missing observed approval message"))?;
        self.message_ids.insert(id);
        Ok(id)
    }

    fn matches_context(&self, reply: ReplyBinding<'_>, now: u64) -> bool {
        !self.closed
            && now < self.expires_at_unix
            && self.request_id == reply.request_id
            && self.owner == reply.owner
            && self.channel == reply.channel
            && self.account.eq_ignore_ascii_case(reply.account)
            && self.self_peer == reply.self_peer
            && self.code.eq_ignore_ascii_case(reply.code)
    }

    pub(super) fn matches_reply(&self, reply: ReplyBinding<'_>, now: u64) -> bool {
        self.matches_context(reply, now)
            && match reply.message_id {
                Some(id) => self.channel.uses_message() && self.message_ids.contains(&id),
                None => !self.channel.uses_message(),
            }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ApprovalPrompt<'a> {
    pub request_id: &'a str,
    pub account: &'a str,
    pub text: &'a str,
    pub allow_always: bool,
    pub deadline: tokio::time::Instant,
}

#[async_trait::async_trait]
pub(crate) trait DialogElicitor: Sync {
    fn verified_owner(&self, account: &str) -> Option<i64>;
    fn has_form_capability(&self) -> bool;
    fn protocol(&self) -> &str;
    async fn elicit(&self, prompt: &ApprovalPrompt<'_>) -> ChannelOutcome;
}

pub(crate) struct SavedReply {
    pub owner: i64,
    pub self_peer: i64,
    pub reply_to_message_id: i32,
    pub text: String,
}

#[async_trait::async_trait]
pub(crate) trait SavedMessagesTransport: Sync {
    fn verified_self_peer(&self, account: &str) -> Option<i64>;
    /// Return Unsupported only when delivery is known not to have occurred.
    async fn send_prompt(
        &self,
        prompt: &ApprovalPrompt<'_>,
        self_peer: i64,
    ) -> std::result::Result<i32, ChannelOutcome>;
    /// Observe a reply using the existing acting client's verified update stream.
    async fn observe_reply(
        &self,
        prompt: &ApprovalPrompt<'_>,
        self_peer: i64,
        message_id: i32,
    ) -> std::result::Result<SavedReply, ChannelOutcome>;
}

#[derive(Default)]
pub(crate) struct ApprovalRoutes<'a> {
    pub dialog: Option<&'a dyn DialogElicitor>,
    pub saved_messages: Option<&'a dyn SavedMessagesTransport>,
}

impl ApprovalRoutes<'_> {
    pub(crate) fn capabilities(&self, account: &str) -> ChannelCapabilities {
        ChannelCapabilities {
            dialog: match self.dialog {
                Some(dialog)
                    if dialog_available(
                        dialog.verified_owner(account).is_some(),
                        dialog.has_form_capability(),
                        dialog.protocol(),
                    ) =>
                {
                    ChannelAvailability::Available
                }
                Some(_) => ChannelAvailability::Unavailable,
                None => ChannelAvailability::Unknown,
            },
            saved_messages: match self.saved_messages {
                Some(transport)
                    if transport
                        .verified_self_peer(account)
                        .is_some_and(|id| id > 0) =>
                {
                    ChannelAvailability::Available
                }
                Some(_) => ChannelAvailability::Unavailable,
                None => ChannelAvailability::Unknown,
            },
            ..Default::default()
        }
    }
}

pub(super) async fn dialog_request(
    dialog: &dyn DialogElicitor,
    prompt: &ApprovalPrompt<'_>,
    seals: &super::sealed::SealedStore,
) -> Result<ChannelOutcome> {
    let Some(owner) = dialog.verified_owner(prompt.account).filter(|id| *id > 0) else {
        return Ok(ChannelOutcome::Unsupported);
    };
    if !dialog_available(true, dialog.has_form_capability(), dialog.protocol()) {
        return Ok(ChannelOutcome::Unsupported);
    }
    let code = format!("{:016X}", rand::random::<u64>());
    let binding = ReplyBinding {
        request_id: prompt.request_id,
        owner,
        channel: ApprovalChannel::DialogChannel,
        account: prompt.account,
        self_peer: None,
        message_id: None,
        code: &code,
    };
    seals.begin(PendingRequest::new(binding, expiry(prompt))?, &[])?;
    let outcome = tokio::time::timeout_at(prompt.deadline, dialog.elicit(prompt))
        .await
        .unwrap_or(ChannelOutcome::TimedOut);
    finish(seals, binding, outcome, prompt.allow_always)
}

pub(super) async fn saved_messages_request(
    transport: &dyn SavedMessagesTransport,
    prompt: &ApprovalPrompt<'_>,
    seals: &super::sealed::SealedStore,
) -> Result<ChannelOutcome> {
    let Some(owner) = transport
        .verified_self_peer(prompt.account)
        .filter(|id| *id > 0)
    else {
        return Ok(ChannelOutcome::Unsupported);
    };
    let code = format!("{:016X}", rand::random::<u64>());
    let choices = if prompt.allow_always {
        "APPROVE, ALWAYS, DENY or CANCEL"
    } else {
        "APPROVE, DENY or CANCEL"
    };
    let body = format!("{}\nReply with {choices} followed by {code}.", prompt.text);
    let delivered = ApprovalPrompt {
        text: &body,
        ..*prompt
    };
    let mut binding = ReplyBinding {
        request_id: prompt.request_id,
        owner,
        channel: ApprovalChannel::SavedMessagesChannel,
        account: prompt.account,
        self_peer: Some(owner),
        message_id: None,
        code: &code,
    };
    seals.begin(PendingRequest::new(binding, expiry(prompt))?, &[&body])?;
    let id =
        match tokio::time::timeout_at(prompt.deadline, transport.send_prompt(&delivered, owner))
            .await
        {
            Ok(Ok(id)) if id > 0 => id,
            Ok(Err(ChannelOutcome::Unsupported)) => {
                return finish(seals, binding, ChannelOutcome::Unsupported, false);
            }
            _ => return finish(seals, binding, ChannelOutcome::AmbiguousDelivery, false),
        };
    binding.message_id = Some(id);
    seals.bind_message(binding, super::unix_seconds())?;
    let outcome =
        match tokio::time::timeout_at(prompt.deadline, transport.observe_reply(prompt, owner, id))
            .await
        {
            Ok(Ok(reply))
                if reply.owner == owner
                    && reply.self_peer == owner
                    && reply.reply_to_message_id == id =>
            {
                reply_decision(&reply.text, &code, prompt.allow_always)
                    .unwrap_or(ChannelOutcome::Denied)
            }
            Ok(Err(ChannelOutcome::Cancelled)) => ChannelOutcome::Cancelled,
            Ok(Err(ChannelOutcome::TimedOut)) | Err(_) => ChannelOutcome::TimedOut,
            _ => ChannelOutcome::AmbiguousDelivery,
        };
    finish(seals, binding, outcome, prompt.allow_always)
}

fn expiry(prompt: &ApprovalPrompt<'_>) -> u64 {
    super::unix_seconds()
        .saturating_add(
            prompt
                .deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .as_secs(),
        )
        .saturating_add(1)
}

fn finish(
    seals: &super::sealed::SealedStore,
    binding: ReplyBinding<'_>,
    outcome: ChannelOutcome,
    allow_always: bool,
) -> Result<ChannelOutcome> {
    let outcome = if outcome == ChannelOutcome::ApprovedAlways && !allow_always {
        ChannelOutcome::Denied
    } else {
        outcome
    };
    if outcome.is_owner_reply() {
        ensure!(
            seals.consume(binding, outcome, super::unix_seconds())?,
            "Approval request is no longer live"
        );
    } else {
        seals.close(binding.request_id, outcome)?;
    }
    Ok(outcome)
}

pub(super) fn reply_decision(text: &str, code: &str, allow_always: bool) -> Option<ChannelOutcome> {
    let (command, supplied) = text.trim().split_once(' ')?;
    if !supplied.eq_ignore_ascii_case(code) {
        return None;
    }
    match command {
        "APPROVE" => Some(ChannelOutcome::ApprovedOnce),
        "ALWAYS" if allow_always => Some(ChannelOutcome::ApprovedAlways),
        "DENY" => Some(ChannelOutcome::Denied),
        "CANCEL" => Some(ChannelOutcome::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/safeguard_channels.rs"]
mod tests;
