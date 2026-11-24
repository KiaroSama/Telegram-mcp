use super::{config_digest, label_for, read_config, session_validate};
use crate::state::State;
use anyhow::{Context, Result, bail};
use rand::RngExt;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn status(name: &str) -> Value {
    json!({"status":name,"repairable":matches!(name,"unauthorized"|"revoked"|"invalid")})
}

fn validate_endpoint(endpoint: &url::Url) -> Result<()> {
    let loopback = match endpoint.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain("localhost")) => true,
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
    Ok(())
}

/// Authenticate a bounded health response without sending any reusable bearer.
pub(crate) async fn authenticated_health_request(
    endpoint: url::Url,
    discovery: &Value,
) -> Result<Value> {
    validate_endpoint(&endpoint)?;
    if discovery["health_protocol"].as_u64() != Some(2) {
        bail!("Shared service health protocol is unverified; restart its owned service");
    }
    let token = discovery["token"]
        .as_str()
        .filter(|token| {
            (32..=4096).contains(&token.len())
                && token.bytes().all(|byte| (33..=126).contains(&byte))
        })
        .context("Invalid service authentication token")?;
    let nonce = rand::rng()
        .random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut target = endpoint.path().to_owned();
    if let Some(query) = endpoint.query() {
        target.push('?');
        target.push_str(query);
    }
    let proof = crate::server::health_request_proof(token, &nonce, &target);
    let client = crate::server::local_http_client(discovery["certificate_pem"].as_str())?;
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut response = client
            .get(endpoint)
            .timeout(Duration::from_secs(25))
            .header("x-telegram-health-nonce", &nonce)
            .header("x-telegram-health-proof", proof)
            .send()
            .await?;
        let status = response.status().as_u16();
        let response_proof = response
            .headers()
            .get("x-telegram-health-proof")
            .and_then(|value| value.to_str().ok())
            .context("Health response has no authentication proof")?
            .to_owned();
        const LIMIT: usize = 65_536;
        if response
            .content_length()
            .is_some_and(|length| length > LIMIT as u64)
        {
            bail!("Health response exceeds its byte limit");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > LIMIT.saturating_sub(body.len()) {
                bail!("Health response exceeds its byte limit");
            }
            body.extend_from_slice(&chunk);
        }
        crate::server::verify_health_response(
            token,
            &nonce,
            &target,
            status,
            &body,
            &response_proof,
        )?;
        if !(200..300).contains(&status) {
            bail!("Authenticated health service returned a failure status");
        }
        Ok(serde_json::from_slice(&body)?)
    })
    .await
    .context("Health request exceeded its deadline")?
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
    if discovery["health_protocol"].as_u64() != Some(2) {
        let mut answer = status("unknown");
        answer["config_digest"] = json!(digest);
        return Ok(answer);
    }
    let mut endpoint = url::Url::parse(
        discovery["url"]
            .as_str()
            .context("Service discovery has no URL")?,
    )?;
    validate_endpoint(&endpoint)?;
    endpoint.set_path("/health");
    endpoint.set_query(None);
    endpoint
        .query_pairs_mut()
        .append_pair("account", &label)
        .append_pair("config_key", key)
        .append_pair("config_digest", &digest);
    let result = async {
        let answer = authenticated_health_request(endpoint, &discovery).await?;
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
