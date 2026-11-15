use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};
use telegram_mcp::{backend::Telegram, tools::local};

/// Each subprocess owns its environment, cwd and synthetic state. It cannot
/// change the parent test harness's process configuration or touch user data.
pub fn isolated(name: &str) -> bool {
    isolated_with(name, &[])
}

pub fn isolated_with(name: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var("LOCAL_DISPATCH_SCENARIO").as_deref() == Ok(name) {
        return true;
    }
    let directory = tempfile::tempdir().unwrap();
    let installation = directory.path().join("installation");
    std::fs::create_dir_all(installation.join("files")).unwrap();
    let allowed = directory.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let mut process = std::process::Command::new(std::env::current_exe().unwrap());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TELEGRAM_") {
            process.env_remove(key);
        }
    }
    let output = process
        .args(["--exact", name, "--nocapture"])
        .current_dir(&installation)
        .env("LOCAL_DISPATCH_SCENARIO", name)
        .envs(environment.iter().copied())
        .env("TELEGRAM_STATE_DIR", directory.path().join("private"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&vec![allowed.to_string_lossy()]).unwrap(),
        )
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[derive(Default)]
pub struct Api {
    pub calls: Mutex<Vec<(String, Value)>>,
    replies: Mutex<HashMap<String, VecDeque<Value>>>,
}
impl Api {
    pub fn reply(&self, method: &str, value: Value) {
        self.replies
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(value);
    }
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}
#[async_trait]
impl Telegram for Api {
    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        if !method.starts_with("local.") {
            telegram_mcp::tl::Schema::global().encode_call(method, &args)?;
        }
        self.calls.lock().unwrap().push((method.into(), args));
        if let Some(value) = self
            .replies
            .lock()
            .unwrap()
            .get_mut(method)
            .and_then(VecDeque::pop_front)
        {
            return Ok(value);
        }
        Ok(match method {
            "local.accounts" => json!({"accounts":[{"label":"work","backend":"grammers"}]}),
            "local.session_health" => json!({"status":"unknown","authorized":null}),
            "local.connection_route" => json!({"route":"direct","verification":"not_probed"}),
            _ => bail!("Unexpected RPC {method}"),
        })
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        Ok(match input.as_str() {
            Some("group") => json!({"_":"inputPeerChat","chat_id":"9"}),
            Some("channel") => json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}),
            Some("me") => json!({"_":"inputPeerSelf"}),
            Some("unsupported") => json!({"_":"inputPeerEmpty"}),
            _ => {
                let id = input
                    .as_i64()
                    .or_else(|| input.as_str().and_then(|v| v.parse().ok()))
                    .unwrap_or(42);
                json!({"_":"inputPeerUser","user_id":id.to_string(),"access_hash":"80"})
            }
        })
    }
}
pub async fn call(api: &Api, name: &str, args: Value) -> Result<Value> {
    local::call(api, name, &args)
        .await
        .expect("The tool must have a native local route")
}
