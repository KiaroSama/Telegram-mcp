//! Native proxy routing. Every failover occurs before an owner's MTProto bytes
//! are forwarded; liveness probes use only a fresh unauthenticated req_pq nonce.
mod gateway;
pub mod mtproxy;
mod pools;
pub mod tunnel;

use crate::{account_manager::ConfigSnapshot, state::State, tools::local::proxies::Proxy};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

pub struct Route {
    pub proxy_url: Option<String>,
    status: Arc<Mutex<Value>>,
    task: Option<JoinHandle<()>>,
}
impl Route {
    pub fn metadata(&self) -> Value {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(json!({"route":"unknown","available":false}))
    }
    pub fn configure(&self, parameters: &mut grammers_mtsender::ConnectionParams) {
        parameters.proxy_url = self.proxy_url.clone();
        parameters.transport = Box::new(|| Box::new(grammers_mtproto::transport::Full::new()));
    }
}
impl Drop for Route {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Candidate {
    pub id: String,
    pub proxy: Option<Proxy>,
}
impl Candidate {
    pub fn describe(&self) -> Value {
        json!({"route":self.id,"kind":self.proxy.as_ref().map(|p|p.kind.as_str()).unwrap_or("direct"),"proxy":self.proxy.is_some()})
    }
}
pub fn candidates(state: &State, snapshot: &ConfigSnapshot, label: &str) -> Result<Vec<Candidate>> {
    pools::candidates(state, snapshot, label)
}

pub async fn route(state: Arc<State>, snapshot: &ConfigSnapshot, label: &str) -> Result<Route> {
    crate::session::validate_label(label)?;
    let check_state = Arc::clone(&state);
    let check_snapshot = ConfigSnapshot {
        values: snapshot.values.clone(),
        digest: snapshot.digest.clone(),
        path: snapshot.path.clone(),
        process_keys: snapshot.process_keys.clone(),
    };
    let check_label = label.to_owned();
    tokio::task::spawn_blocking(move || candidates(&check_state, &check_snapshot, &check_label))
        .await
        .context("Proxy configuration worker failed")??;
    let status = Arc::new(Mutex::new(
        json!({"route":"direct","proxy":false,"verification":"not_probed","account":label}),
    ));
    gateway::start(state, snapshot, label.to_owned(), status).await
}

/// A protocol-level reachability check, never using an account's authorization.
pub async fn probe(proxy: &Proxy, target: SocketAddr, dc: i16) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = tunnel::connect(Some(proxy), target, dc).await?;
        gateway::probe_stream(&mut stream).await
    })
    .await
    .context("Proxy Telegram transport probe timed out")?
}

pub(crate) fn validate(proxy: &Proxy) -> Result<()> {
    pools::validate(proxy)
}

pub(crate) struct ContextState {
    pub state: Arc<State>,
    pub label: String,
    pub snapshot: ConfigSnapshot,
    pub status: Arc<Mutex<Value>>,
    pub failed_until: Mutex<std::collections::HashMap<SocketAddr, Instant>>,
}
impl ContextState {
    pub fn record(&self, candidate: &Candidate, latency: u64, target: SocketAddr) -> Result<()> {
        let mut metadata = candidate.describe();
        metadata["account"] = json!(self.label);
        metadata["verification"] = json!("telegram_transport_verified");
        metadata["latency_ms"] = json!(latency);
        metadata["destination"] = json!(target.to_string());
        *self
            .status
            .lock()
            .map_err(|_| anyhow::anyhow!("Proxy route status is unavailable"))? = metadata.clone();
        self.state
            .update_json("proxy-routes-v1.json", json!({}), |store| {
                let rows = store
                    .as_object_mut()
                    .context("Proxy route state is invalid")?;
                rows.insert(self.label.clone(), metadata);
                Ok(())
            })?;
        if candidate.proxy.is_some() {
            self.state
                .update_json("proxies-v1.json", json!({}), |store| {
                    if let Some(record) = store.get_mut(&candidate.id) {
                        record["healthy"] = json!(true);
                        record["last_result"] = json!("telegram_transport_reachable");
                        record["latency_ms"] = json!(latency);
                    }
                    Ok(())
                })?;
        }
        Ok(())
    }
    pub fn record_failure(&self, candidate: &Candidate) {
        if candidate.proxy.is_some() {
            let _ = self
                .state
                .update_json("proxies-v1.json", json!({}), |store| {
                    if let Some(record) = store.get_mut(&candidate.id) {
                        record["healthy"] = json!(false);
                        record["last_result"] = json!("connection_failed");
                    }
                    Ok(())
                });
        }
    }
}

/// Pool membership helper shared with local management tools after legacy import.
pub fn record_in_pool(record: &Value, pool: &str) -> bool {
    pools::in_pool(record, pool)
}
