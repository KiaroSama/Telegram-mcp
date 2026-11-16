use super::{config_digest, label_for, read_config, session_validate};
use crate::state::State;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn status(name: &str) -> Value {
    json!({"status":name,"repairable":matches!(name,"unauthorized"|"revoked"|"invalid")})
}

pub async fn session_health(env_file: &Path, key: &str) -> Result<Value> {
    let state = State::global()?;
    session_health_with_state(&state, env_file, key).await
}

/// Explicit state ownership keeps health tests and embedders off ambient user data.
pub async fn session_health_with_state(state: &State, env_file: &Path, key: &str) -> Result<Value> {
    let label = label_for(key)?;
    let snapshot = read_config(env_file)?;
    let Some(value) = snapshot.values.get(key).filter(|v| !v.is_empty()) else {
        return Ok(status("invalid"));
    };
    let digest = config_digest(value);
    if key.starts_with("TELEGRAM_SESSION_STRING") && session_validate(value) != "valid" {
        return Ok(status("invalid"));
    }
    if key.starts_with("TELEGRAM_SESSION_NAME")
        && !state.root().join(format!("{label}.session.json")).is_file()
        && ![
            snapshot.path.parent().unwrap_or(Path::new(".")),
            state.root(),
        ]
        .into_iter()
        .flat_map(|base| {
            let path = base.join(value);
            [
                path.clone(),
                std::path::PathBuf::from(format!("{}.session", path.display())),
            ]
        })
        .any(|path| path.is_file())
    {
        return Ok(status("invalid"));
    }
    let discovery = state.read_json("http-service.json", Value::Null)?;
    if discovery.is_null() {
        let mut answer = status("unknown");
        answer["config_digest"] = json!(digest);
        return Ok(answer);
    }
    let mut endpoint = url::Url::parse(
        discovery["url"]
            .as_str()
            .context("Service discovery has no URL")?,
    )?;
    let loopback = match endpoint.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !matches!(endpoint.scheme(), "http" | "https")
        || !loopback
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        bail!("Private service discovery must name a loopback HTTP or HTTPS endpoint");
    }
    let token = discovery["token"]
        .as_str()
        .context("Service discovery has no authentication token")?;
    if token.len() < 32 || token.chars().any(|c| c.is_control() || c.is_whitespace()) {
        bail!("Invalid service authentication token");
    }
    endpoint.set_path("/health");
    endpoint.set_query(None);
    endpoint
        .query_pairs_mut()
        .append_pair("account", &label)
        .append_pair("config_key", key)
        .append_pair("config_digest", &digest);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()?;
    let result = async {
        let response = client.get(endpoint).bearer_auth(token).send().await?;
        let response = response.error_for_status()?;
        if response
            .content_length()
            .is_some_and(|length| length > 65_536)
        {
            bail!("Health response exceeds its byte limit");
        }
        let bytes = response.bytes().await?;
        if bytes.len() > 65_536 {
            bail!("Health response exceeds its byte limit");
        }
        let answer: Value = serde_json::from_slice(&bytes)?;
        let name = answer["status"]
            .as_str()
            .context("Health response has no authorization status")?;
        if ![
            "healthy",
            "unauthorized",
            "revoked",
            "invalid",
            "network",
            "unknown",
            "busy",
        ]
        .contains(&name)
        {
            bail!("Unrecognized authorization status");
        }
        // A different active generation cannot testify about this configuration.
        if answer["config_digest"].as_str() != Some(&digest)
            || answer["config_key"].as_str() != Some(key)
        {
            return Ok::<Value, anyhow::Error>(status("unknown"));
        }
        let mut result = status(name);
        if let Some(id) = answer.get("user_id") {
            result["user_id"] = id.clone();
        }
        Ok(result)
    }
    .await;
    let mut answer = result.unwrap_or_else(|_| status("busy"));
    answer["config_digest"] = json!(digest);
    Ok(answer)
}
