//! Metadata-only event cursors for resumable chat integrations.
use super::State;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::Duration;

pub struct EventLog {
    capacity: usize,
    boot: String,
    sequence: u64,
    events: VecDeque<Value>,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new(2000)
    }
}
impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            boot: format!("{:016x}", rand::random::<u64>()),
            sequence: 0,
            events: VecDeque::new(),
        }
    }
}

#[derive(Default)]
pub struct EventQuery {
    pub after_seq: i64,
    pub boot: Option<String>,
    pub account: Option<String>,
    pub chat_id: Option<String>,
    pub timeout: Duration,
    pub limit: usize,
}

impl State {
    /// The authenticated callback supplies only message metadata. This projector
    /// discards text, media references and unexpected fields even if supplied.
    pub fn record_message_event(&self, value: Value) -> Result<()> {
        let kind = value["chat_type"]
            .as_str()
            .context("An event needs its chat type")?;
        if !["private", "group", "supergroup"].contains(&kind) {
            return Ok(());
        }
        let account = value["account"]
            .as_str()
            .context("An event needs an account label")?;
        if account.is_empty()
            || account.len() > 64
            || !account
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            bail!("An event has invalid account metadata");
        }
        for field in ["chat_id", "message_id", "sender_id", "topic", "reply_to"] {
            if value[field].is_null() {
                if matches!(field, "chat_id" | "message_id") {
                    bail!("An event needs its chat and message identity");
                }
                continue;
            }
            let id = value[field]
                .as_i64()
                .or_else(|| {
                    value[field]
                        .as_str()
                        .and_then(|text| text.parse::<i64>().ok())
                })
                .context("An event has invalid numeric metadata")?;
            if matches!(field, "message_id" | "topic" | "reply_to")
                && !(1..=i32::MAX as i64).contains(&id)
            {
                bail!("An event has an invalid message identity");
            }
        }
        for field in ["sender_name", "date", "_chat_username"] {
            if let Some(raw) = value.get(field).filter(|raw| !raw.is_null()) {
                let text = raw
                    .as_str()
                    .context("An event has invalid string metadata")?;
                if text.chars().count() > 512 {
                    bail!("Event string metadata exceeds its budget");
                }
            }
        }
        for field in [
            "sender_bot",
            "out",
            "addressed",
            "addressed_unknown",
            "service",
        ] {
            if value
                .get(field)
                .is_some_and(|raw| !raw.is_null() && !raw.is_boolean())
            {
                bail!("An event has invalid boolean metadata");
            }
        }
        let mut record = serde_json::Map::new();
        for key in [
            "account",
            "chat_id",
            "chat_type",
            "message_id",
            "sender_id",
            "sender_name",
            "sender_bot",
            "out",
            "addressed",
            "addressed_unknown",
            "topic",
            "reply_to",
            "service",
            "date",
            "_chat_username",
        ] {
            if let Some(value) = value.get(key).filter(|value| !value.is_null()) {
                record.insert(key.to_owned(), value.clone());
            }
        }
        let mut log = self
            .message_events
            .lock()
            .map_err(|_| anyhow::anyhow!("Message event log lock is unavailable"))?;
        if log.capacity == 0 {
            return Ok(());
        }
        log.sequence = log.sequence.checked_add(1).unwrap_or_else(|| {
            log.boot = format!("{:016x}", rand::random::<u64>());
            log.events.clear();
            1
        });
        record.insert("seq".into(), json!(log.sequence));
        log.events.push_back(Value::Object(record));
        while log.events.len() > log.capacity {
            log.events.pop_front();
        }
        drop(log);
        self.changed.notify_waiters();
        Ok(())
    }

    pub async fn wait_message_events(
        &self,
        query: &EventQuery,
        allow: &(dyn Fn(&Value) -> bool + Send + Sync),
    ) -> Result<Value> {
        if query.after_seq < -1 {
            bail!("after_seq must be -1 or nonnegative");
        }
        if query.timeout > Duration::from_secs(50) {
            bail!("Event wait must not exceed 50 seconds");
        }
        if !(1..=500).contains(&query.limit) {
            bail!("Event batch limit must be between 1 and 500");
        }
        let deadline = tokio::time::Instant::now() + query.timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let log = self
                    .message_events
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Message event log lock is unavailable"))?;
                if query.after_seq < 0 {
                    return Ok(
                        json!({"boot":log.boot,"gap":false,"last_seq":log.sequence,"events":[],"event_log_disabled":log.capacity==0}),
                    );
                }
                let after = query.after_seq as u64;
                if query.boot.as_ref().is_some_and(|boot| *boot != log.boot) || after > log.sequence
                {
                    return Ok(
                        json!({"boot":log.boot,"reset":true,"gap":true,"last_seq":log.sequence,"events":[]}),
                    );
                }
                let oldest = log
                    .events
                    .front()
                    .and_then(|value| value["seq"].as_u64())
                    .unwrap_or(log.sequence.saturating_add(1));
                let gap = after.saturating_add(1) < oldest && after < log.sequence;
                let mut events = Vec::new();
                for event in &log.events {
                    if event["seq"].as_u64().unwrap_or(0) <= after {
                        continue;
                    }
                    if query.account.as_ref().is_some_and(|account| {
                        event["account"]
                            .as_str()
                            .is_none_or(|label| !label.eq_ignore_ascii_case(account))
                    }) {
                        continue;
                    }
                    if query
                        .chat_id
                        .as_ref()
                        .is_some_and(|chat| super::normalize_key(&event["chat_id"]) != *chat)
                    {
                        continue;
                    }
                    if !allow(event) {
                        continue;
                    }
                    let mut public = event.as_object().cloned().unwrap_or_default();
                    public.retain(|key, _| !key.starts_with('_'));
                    events.push(Value::Object(public));
                    if events.len() >= query.limit {
                        break;
                    }
                }
                if !events.is_empty() || gap || tokio::time::Instant::now() >= deadline {
                    let last = if events.len() >= query.limit {
                        events
                            .last()
                            .and_then(|event| event["seq"].as_u64())
                            .unwrap_or(log.sequence)
                    } else {
                        log.sequence
                    };
                    return Ok(
                        json!({"boot":log.boot,"gap":gap,"last_seq":last,"events":events,"event_log_disabled":log.capacity==0}),
                    );
                }
            }
            tokio::select! {_=&mut notified=>{},_=tokio::time::sleep_until(deadline)=>{}}
        }
    }
}
