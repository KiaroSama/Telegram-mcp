//! Incoming event feed handlers honoring `TELEGRAM_EVENT_FEED_FILE`, live
//! `settle_ms` reconfiguration, pre-touching on enable, and quoted watch commands.
use crate::state::State;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub fn incoming_feed_status(state: &State) -> Result<Value> {
    state.incoming_feed_status()
}

pub fn disable_incoming_feed(state: &State) -> Result<Value> {
    state.disable_incoming_feed()
}

pub fn enable_incoming_feed(state: &Arc<State>, args: &Value) -> Result<Value> {
    let settle_ms = crate::args::limit(args, "settle_ms", 6000, 50_000)? as u64;
    state.enable_incoming_feed(settle_ms)
}
