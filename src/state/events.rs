use super::State;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_PENDING: usize = 500;
const MAX_PENDING_AGE: Duration = Duration::from_secs(3600);

pub(crate) fn positive_limit(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 1. && *value <= u64::MAX as f64)
        .map(|value| value as u64)
        .unwrap_or(default)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingEvent {
    pub account: String,
    pub chat_id: String,
    pub message_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

struct Burst {
    event: IncomingEvent,
    first_id: String,
    count: u64,
    first: Instant,
    last: Instant,
}

struct EventVisibility<F> {
    permitted: F,
    include_global_drop_count: bool,
}

#[derive(Default)]
pub struct EventJournal {
    bursts: BTreeMap<(String, String), Burst>,
    dropped: u64,
    drop_reasons: BTreeMap<String, u64>,
    recent_dropped: VecDeque<Value>,
    feed_enabled: bool,
    generation: u64,
    settle_ms: u64,
    error: Option<String>,
}

impl EventJournal {
    fn expire(&mut self) {
        let age = Duration::from_secs(positive_limit(
            "TELEGRAM_EVENT_PENDING_TTL_SECONDS",
            MAX_PENDING_AGE.as_secs(),
        ));
        let expired = self
            .bursts
            .iter()
            .filter(|(_, burst)| burst.last.elapsed() >= age)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            self.bursts.remove(&key);
            self.note_drop(&key, "expired");
        }
    }

    fn note_drop(&mut self, key: &(String, String), reason: &str) {
        self.dropped = self.dropped.saturating_add(1);
        *self.drop_reasons.entry(reason.to_owned()).or_default() += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.recent_dropped
            .push_back(json!({"account":key.0,"chat_id":key.1,"reason":reason,"at":now}));
        while self.recent_dropped.len() > 20 {
            self.recent_dropped.pop_front();
        }
    }

    fn summary(burst: &Burst) -> Value {
        json!({"event":true,"account":burst.event.account,"chat_id":burst.event.chat_id,
            "name":burst.event.name,"username":burst.event.username,"bot":burst.event.bot,
            "message_count":burst.count,"count":burst.count,"first_message_id":burst.first_id,
            "last_message_id":burst.event.message_id,"burst_seconds":burst.last.duration_since(burst.first).as_secs_f64()})
    }
}

impl State {
    /// Called only by the authenticated update stream after incoming/private filtering.
    pub fn record_incoming(self: &Arc<Self>, event: IncomingEvent) -> Result<()> {
        let autostart_generation = self.journal_incoming(event)?;
        self.changed.notify_waiters();
        if let Some(generation) = autostart_generation {
            self.enable_incoming_feed_at_generation(6000, Some(generation))?;
        }
        Ok(())
    }

    fn journal_incoming(&self, event: IncomingEvent) -> Result<Option<u64>> {
        if event.account.is_empty() || event.chat_id.is_empty() || event.message_id.is_empty() {
            bail!("An incoming event needs an account, chat and message id");
        }
        if event.account.len() > 64
            || event.chat_id.len() > 32
            || event.message_id.len() > 32
            || event.name.chars().count() > 512
            || event
                .username
                .as_ref()
                .is_some_and(|username| username.len() > 64)
        {
            bail!("Incoming event metadata exceeds its field budgets");
        }
        let mut journal = self
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
        journal.expire();
        let key = (event.account.to_lowercase(), event.chat_id.clone());
        let maximum = positive_limit("TELEGRAM_EVENT_PENDING_MAX", MAX_PENDING as u64) as usize;
        if !journal.bursts.contains_key(&key) && journal.bursts.len() >= maximum {
            let oldest = journal
                .bursts
                .iter()
                .min_by_key(|(_, burst)| burst.last)
                .map(|(key, _)| key.clone());
            if let Some(key) = oldest {
                journal.bursts.remove(&key);
                journal.note_drop(&key, "overflow");
            }
        }
        let now = Instant::now();
        journal
            .bursts
            .entry(key)
            .and_modify(|burst| {
                burst.count = burst.count.saturating_add(1);
                burst.last = now;
                burst.event = event.clone();
            })
            .or_insert_with(|| Burst {
                first_id: event.message_id.clone(),
                event,
                count: 1,
                first: now,
                last: now,
            });
        let autostart = self
            .feed_autostart
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        Ok(autostart.then_some(journal.generation))
    }

    pub async fn wait_incoming(
        &self,
        account: Option<&str>,
        chat: Option<&str>,
        timeout: Duration,
        settle: Option<Duration>,
        limit: usize,
    ) -> Result<Value> {
        self.wait_incoming_view(
            account,
            chat,
            timeout,
            settle,
            limit,
            EventVisibility {
                permitted: |_: &IncomingEvent| true,
                include_global_drop_count: true,
            },
        )
        .await
    }

    /// Apply the caller's access policy before selecting or consuming a burst.
    /// Historical global drop counters cannot be attributed safely and are omitted.
    pub async fn wait_incoming_permitted(
        &self,
        account: Option<&str>,
        chat: Option<&str>,
        timeout: Duration,
        settle: Option<Duration>,
        limit: usize,
        permitted: impl Fn(&IncomingEvent) -> bool + Send + Sync,
    ) -> Result<Value> {
        self.wait_incoming_view(
            account,
            chat,
            timeout,
            settle,
            limit,
            EventVisibility {
                permitted,
                include_global_drop_count: false,
            },
        )
        .await
    }

    async fn wait_incoming_view<F>(
        &self,
        account: Option<&str>,
        chat: Option<&str>,
        timeout: Duration,
        settle: Option<Duration>,
        limit: usize,
        visibility: EventVisibility<F>,
    ) -> Result<Value>
    where
        F: Fn(&IncomingEvent) -> bool + Send + Sync,
    {
        if timeout > Duration::from_secs(50) || timeout.is_zero() {
            bail!("The wait timeout must be between 0 and 50 seconds");
        }
        if !(1..=100).contains(&limit) {
            bail!("The event limit must be between 1 and 100");
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let remaining = {
                let mut journal = self
                    .events
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
                journal.expire();
                let wanted = |key: &(String, String), burst: &Burst| {
                    account.is_none_or(|a| key.0 == a.to_lowercase())
                        && chat.is_none_or(|c| key.1 == c)
                        && !(chat.is_none() && burst.event.bot)
                        && (visibility.permitted)(&burst.event)
                };
                if let Some(settle) = settle {
                    let found = journal
                        .bursts
                        .iter()
                        .find(|(key, burst)| wanted(key, burst) && burst.last.elapsed() >= settle)
                        .map(|(key, _)| key.clone());
                    if let Some(key) = found {
                        let burst = journal
                            .bursts
                            .remove(&key)
                            .expect("Selected pending event exists");
                        return Ok(EventJournal::summary(&burst));
                    }
                    journal
                        .bursts
                        .iter()
                        .filter(|(key, burst)| wanted(key, burst))
                        .map(|(_, burst)| settle.saturating_sub(burst.last.elapsed()))
                        .min()
                } else {
                    let pending: Vec<_> = journal
                        .bursts
                        .iter()
                        .filter(|(key, burst)| wanted(key, burst))
                        .map(|(_, burst)| EventJournal::summary(burst))
                        .collect();
                    if !pending.is_empty() {
                        let mut result = json!({"event":true,"total":pending.len(),"has_more":pending.len()>limit,"pending_chats":pending.into_iter().take(limit).collect::<Vec<_>>()});
                        if visibility.include_global_drop_count {
                            result["dropped_total"] = json!(journal.dropped);
                        }
                        return Ok(result);
                    }
                    None
                }
            };
            if tokio::time::Instant::now() >= deadline {
                return Ok(json!({"event":false,"reason":"timeout","waiting_for":chat}));
            }
            let wake = remaining
                .map(|delay| tokio::time::Instant::now() + delay)
                .unwrap_or(deadline)
                .min(deadline);
            tokio::select! {_=&mut notified=>{},_=tokio::time::sleep_until(wake)=>{}}
        }
    }

    pub fn incoming_feed_status(&self) -> Result<Value> {
        let journal = self
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
        Ok(
            json!({"enabled":journal.feed_enabled,"running":journal.feed_enabled,"settle_ms":journal.settle_ms,
            "pending_chats":journal.bursts.len(),"dropped_total":journal.dropped,"dropped_reason_counts":journal.drop_reasons,"recent_dropped":journal.recent_dropped,"error":journal.error,
            "path":self.root.join("incoming_feed.jsonl"),"autostart_pending":self.feed_autostart.load(std::sync::atomic::Ordering::Relaxed),"max_bytes":positive_limit("TELEGRAM_EVENT_FEED_MAX_BYTES",8_388_608),"max_age_seconds":positive_limit("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS",604_800)}),
        )
    }

    pub fn disable_incoming_feed(&self) -> Result<Value> {
        let mut journal = self
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
        self.feed_autostart
            .store(false, std::sync::atomic::Ordering::Relaxed);
        journal.feed_enabled = false;
        journal.generation = journal.generation.wrapping_add(1);
        drop(journal);
        self.changed.notify_waiters();
        self.incoming_feed_status()
    }

    pub fn enable_incoming_feed(self: &Arc<Self>, settle_ms: u64) -> Result<Value> {
        self.enable_incoming_feed_at_generation(settle_ms, None)
    }

    fn enable_incoming_feed_at_generation(
        self: &Arc<Self>,
        settle_ms: u64,
        expected_generation: Option<u64>,
    ) -> Result<Value> {
        if !(1..=50_000).contains(&settle_ms) {
            bail!("settle_ms must be between 1 and 50000");
        }
        let runtime = tokio::runtime::Handle::try_current()
            .context("Incoming feed requires an async runtime")?;
        let mut journal = self
            .events
            .lock()
            .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
        // A disable or manual enable after this event's autostart claim wins.
        if expected_generation.is_some_and(|expected| journal.generation != expected) {
            drop(journal);
            return self.incoming_feed_status();
        }
        self.feed_autostart
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if journal.feed_enabled {
            if journal.settle_ms != settle_ms {
                bail!("Disable the running feed before changing its settle period");
            }
            drop(journal);
            return self.incoming_feed_status();
        }
        journal.feed_enabled = true;
        journal.settle_ms = settle_ms;
        journal.generation = journal.generation.wrapping_add(1);
        let generation = journal.generation;
        drop(journal);
        let state = Arc::clone(self);
        runtime.spawn(async move {
            state
                .feed_loop(generation, Duration::from_millis(settle_ms))
                .await;
        });
        self.incoming_feed_status()
    }

    async fn feed_loop(self: Arc<Self>, generation: u64, settle: Duration) {
        let mut maintenance = Instant::now();
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let outcome = (|| -> Result<bool> {
                let mut journal = self
                    .events
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Event journal lock is unavailable"))?;
                if !journal.feed_enabled || journal.generation != generation {
                    return Ok(false);
                }
                journal.expire();
                if maintenance.elapsed() >= Duration::from_secs(300) {
                    self.maintain_event_feed()?;
                    maintenance = Instant::now();
                }
                let key = journal
                    .bursts
                    .iter()
                    .find(|(_, burst)| !burst.event.bot && burst.last.elapsed() >= settle)
                    .map(|(key, _)| key.clone());
                if let Some(key) = key {
                    let summary = EventJournal::summary(
                        journal
                            .bursts
                            .get(&key)
                            .context("Pending event disappeared")?,
                    );
                    // Remove only after durable append. A disk failure keeps the burst pending.
                    self.append_event_line(&summary)?;
                    journal.bursts.remove(&key);
                    journal.error = None;
                }
                Ok(true)
            })();
            match outcome {
                Ok(false) => break,
                Err(_) => {
                    if let Ok(mut journal) = self.events.lock() {
                        journal.error = Some(
                            "Event feed cannot write private state; pending events were retained"
                                .to_owned(),
                        );
                    }
                }
                _ => {}
            }
            tokio::select! {_=&mut notified=>{},_=tokio::time::sleep(settle.min(Duration::from_secs(1)))=>{}}
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/state_review.rs"]
mod review_tests;
