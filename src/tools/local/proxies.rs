use crate::backend::Telegram;
use crate::state::State;
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::Instant;
mod source;
pub use source::source_selector;

#[cfg(test)]
#[path = "../../../tests/unit/local_proxy.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
pub struct Proxy {
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub secret: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Proxy {
    pub fn fingerprint(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self)?;
        Ok(format!("{:x}", Sha256::digest(bytes))[..20].to_owned())
    }
    pub fn describe(&self) -> Result<Value> {
        Ok(json!({"id":self.fingerprint()?,"kind":self.kind,"host":self.host,"port":self.port}))
    }
}

pub fn parse_proxy(input: &str) -> Result<Proxy> {
    let line = input
        .trim()
        .trim_matches(['<', '>', '"', '\''])
        .trim_end_matches([',', ';']);
    let (kind, host, port, secret, username, password) =
        if line.contains("://") || line.starts_with("t.me/") || line.starts_with("telegram.me/") {
            let candidate = if line.contains("://") {
                line.to_owned()
            } else {
                format!("https://{line}")
            };
            let url = url::Url::parse(&candidate).context("Invalid proxy URL")?;
            if url.scheme() == "tg"
                || matches!(
                    url.host_str(),
                    Some("t.me" | "telegram.me" | "www.t.me" | "www.telegram.me")
                )
            {
                let route = if url.scheme() == "tg" {
                    url.host_str().unwrap_or("")
                } else {
                    url.path().trim_matches('/')
                };
                if !matches!(route, "proxy" | "socks") {
                    bail!("Not a Telegram proxy URL");
                }
                let query = url.query().unwrap_or("").replace('+', "%2B");
                let params: std::collections::HashMap<_, _> =
                    url::form_urlencoded::parse(query.as_bytes())
                        .into_owned()
                        .collect();
                (
                    if route == "proxy" {
                        "mtproto"
                    } else {
                        "socks5"
                    }
                    .to_owned(),
                    params
                        .get("server")
                        .cloned()
                        .context("Proxy host is missing")?,
                    params
                        .get("port")
                        .context("Proxy port is missing")?
                        .parse::<u16>()
                        .context("Invalid proxy port")?,
                    params.get("secret").cloned(),
                    params.get("user").cloned(),
                    params.get("pass").cloned(),
                )
            } else {
                let kind = match url.scheme() {
                    "socks" | "socks5" | "socks5h" => "socks5",
                    "socks4" | "socks4a" => "socks4",
                    "http" => "http",
                    _ => bail!("Unsupported proxy protocol"),
                };
                (
                    kind.to_owned(),
                    url.host_str().context("Proxy host is missing")?.to_owned(),
                    url.port_or_known_default()
                        .context("Proxy port is missing")?,
                    None,
                    if url.username().is_empty() {
                        None
                    } else {
                        Some(decode_component(url.username())?)
                    },
                    url.password().map(decode_component).transpose()?,
                )
            }
        } else {
            let mut pieces = line.rsplitn(3, ':');
            let secret = pieces.next().context("Proxy secret is missing")?;
            let port = pieces
                .next()
                .context("Proxy port is missing")?
                .parse::<u16>()
                .context("Invalid proxy port")?;
            let host = pieces
                .next()
                .context("Proxy host is missing")?
                .trim_matches(['[', ']'])
                .to_owned();
            (
                "mtproto".to_owned(),
                host,
                port,
                Some(secret.to_owned()),
                None,
                None,
            )
        };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || host.chars().any(char::is_whitespace) || port == 0 {
        bail!("Invalid proxy host or port");
    }
    if username
        .as_deref()
        .into_iter()
        .chain(password.as_deref())
        .any(|value| value.chars().any(char::is_control))
    {
        bail!("Proxy credentials contain control characters");
    }
    if kind == "mtproto" {
        let secret = secret
            .as_deref()
            .context("MTProto proxy secret is missing")?;
        if secret.len() > 1024 {
            bail!("MTProto proxy secret exceeds its byte limit");
        }
        let bytes = if secret.len() % 2 == 0 && secret.chars().all(|c| c.is_ascii_hexdigit()) {
            (0..secret.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&secret[i..i + 2], 16))
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(secret.trim_end_matches('='))
                .or_else(|_| {
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(secret.trim_end_matches('='))
                })
                .context("Invalid proxy secret encoding")?
        };
        if !(bytes.len() == 16
            || bytes.len() == 17 && bytes[0] == 0xdd
            || bytes.len() >= 18 && bytes[0] == 0xee)
        {
            bail!("Invalid MTProto proxy secret length or prefix");
        }
    }
    Ok(Proxy {
        kind,
        host,
        port,
        secret,
        username,
        password,
    })
}

fn decode_component(text: &str) -> Result<String> {
    let mut decoded = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(
                bytes
                    .get(index + 1..index + 3)
                    .context("Incomplete proxy URL escape")?,
            )?;
            decoded.push(u8::from_str_radix(hex, 16).context("Invalid proxy URL escape")?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("Proxy URL credentials must be UTF-8")
}

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if ![
        "add_proxies",
        "list_proxies",
        "remove_proxies",
        "test_proxies",
        "get_connection_route",
    ]
    .contains(&name)
    {
        return None;
    }
    Some(dispatch(api, name, args).await)
}

async fn dispatch(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let state = State::global()?;
    let pool = args["pool"].as_str().map(str::to_lowercase);
    if name == "get_connection_route" {
        return api
            .invoke("local.connection_route", json!({"account":args["account"]}))
            .await;
    }
    let selected = |record: &Value| {
        pool.as_ref().is_none_or(|pool| {
            record["pool"].as_str() == Some(pool)
                || record["pools"]
                    .as_array()
                    .is_some_and(|pools| pools.iter().any(|member| member.as_str() == Some(pool)))
        })
    };
    match name {
        "list_proxies" => {
            let store = state.read_json("proxies-v1.json", json!({}))?;
            let mut rows = Vec::new();
            for record in store
                .as_object()
                .context("Proxy pool is invalid")?
                .values()
                .filter(|record| selected(record))
            {
                let proxy: Proxy = serde_json::from_value(record["proxy"].clone())
                    .context("Stored proxy is invalid")?;
                let mut row = proxy.describe()?;
                row["pool"] = record["pool"].clone();
                row["pools"] = record["pools"].clone();
                row["healthy"] = record["healthy"].clone();
                row["last_result"] = record["last_result"].clone();
                row["latency_ms"] = record["latency_ms"].clone();
                rows.push(row);
            }
            Ok(json!({"proxies":rows}))
        }
        "add_proxies" => {
            let mut text = args["text"].as_str().unwrap_or("").to_owned();
            if let Some(source) = args.get("source").filter(|source| !source.is_null()) {
                let peer = api.resolve_peer(&source_selector(source)?).await?;
                let result=api.invoke("messages.getHistory",json!({"peer":peer,"offset_id":0,"offset_date":0,"add_offset":0,"limit":200,"max_id":0,"min_id":0,"hash":"0"})).await?;
                collect_proxy_text(&result, &mut text);
            }
            if text.trim().is_empty() {
                bail!("Give text containing proxy links or a source channel");
            }
            let mut found = Vec::new();
            let mut invalid = Vec::new();
            for token in text.split_whitespace().take(10_000) {
                if !(token.contains("://")
                    || token.starts_with("t.me/")
                    || token.matches(':').count() >= 2)
                {
                    continue;
                }
                match parse_proxy(token) {
                    Ok(proxy) => found.push(proxy),
                    Err(_) => invalid.push(json!({"reason":"invalid_proxy_entry"})),
                }
            }
            let pool = pool.unwrap_or_else(|| "shared".to_owned());
            let (added,duplicates)=state.update_json("proxies-v1.json",json!({}),|store| {
                let map=store.as_object_mut().context("Proxy pool is invalid")?;
                let mut added=Vec::new();let mut duplicates=0;
                for proxy in &found {
                    let id=proxy.fingerprint()?;
                    if let Some(existing)=map.get_mut(&id){
                        if !existing.is_object()||!existing["pool"].is_string()||serde_json::from_value::<Proxy>(existing["proxy"].clone()).is_err(){
                            bail!("Stored proxy is invalid; existing bytes were preserved");
                        }
                        let mut memberships=existing["pools"].as_array().cloned().unwrap_or_else(||vec![existing["pool"].clone()]);
                        if !memberships.contains(&json!(pool)){memberships.push(json!(pool));}
                        existing["pools"]=json!(memberships);
                        duplicates+=1;continue;
                    }
                    if map.len()>=4096{bail!("Proxy pool reached its limit of 4096 entries");}
                    map.insert(id,json!({"proxy":proxy,"pool":pool,"pools":[pool],"healthy":null,"last_result":"not_tested"}));
                    added.push(proxy.clone());
                }
                Ok((added,duplicates))
            })?;
            let results = probe_many(&state, &added).await?;
            Ok(json!({"added":results,"duplicates":duplicates,"invalid":invalid}))
        }
        "remove_proxies" => {
            let ids = string_ids(args.get("ids"))?;
            let unhealthy = crate::args::boolean(args, "unhealthy", false)?;
            if ids.is_empty() && !unhealthy {
                bail!("Give proxy ids or unhealthy=true");
            }
            let removed = state.update_json("proxies-v1.json", json!({}), |store| {
                let map = store.as_object_mut().context("Proxy pool is invalid")?;
                let mut removed = 0;
                map.retain(|id, record| {
                    if !selected(record)
                        || !(ids.contains(id) || unhealthy && record["healthy"] == json!(false))
                    {
                        return true;
                    }
                    removed += 1;
                    if let Some(pool) = &pool {
                        let mut memberships = record["pools"]
                            .as_array()
                            .cloned()
                            .unwrap_or_else(|| vec![record["pool"].clone()]);
                        memberships.retain(|member| member.as_str() != Some(pool));
                        if !memberships.is_empty() {
                            record["pool"] = memberships[0].clone();
                            record["pools"] = json!(memberships);
                            return true;
                        }
                    }
                    false
                });
                Ok(removed)
            })?;
            Ok(json!({"removed":removed}))
        }
        "test_proxies" => {
            let ids = string_ids(args.get("ids"))?;
            let store = state.read_json("proxies-v1.json", json!({}))?;
            let mut chosen = store
                .as_object()
                .context("Proxy pool is invalid")?
                .iter()
                .filter(|(id, record)| selected(record) && (ids.is_empty() || ids.contains(*id)))
                .map(|(id, record)| {
                    serde_json::from_value::<Proxy>(record["proxy"].clone())
                        .map(|proxy| (record["probe_sequence"].as_u64().unwrap_or(0), id, proxy))
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // Missing sequence means deferred/never tested. Thereafter use the
            // least recently tested entries so every repeated batch advances.
            chosen.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
            let chosen = chosen
                .into_iter()
                .map(|(_, _, proxy)| proxy)
                .collect::<Vec<_>>();
            Ok(json!({"results":probe_many(&state,&chosen).await?}))
        }
        _ => bail!("Unknown proxy tool"),
    }
}

fn string_ids(value: Option<&Value>) -> Result<HashSet<String>> {
    match value {
        None | Some(Value::Null) => Ok(HashSet::new()),
        Some(Value::Array(rows)) => rows
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .context("Proxy ids must be strings")
            })
            .collect(),
        _ => bail!("Proxy ids must be an array"),
    }
}

fn collect_proxy_text(value: &Value, text: &mut String) {
    match value {
        Value::Array(rows) => {
            for row in rows {
                collect_proxy_text(row, text);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                if ["message", "url", "text"].contains(&key.as_str()) {
                    if let Some(value) = value.as_str() {
                        text.push('\n');
                        text.push_str(value);
                    }
                } else if value.is_object() || value.is_array() {
                    collect_proxy_text(value, text);
                }
            }
        }
        _ => {}
    }
}

async fn probe_many(state: &State, proxies: &[Proxy]) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut tasks = tokio::task::JoinSet::new();
    // Bound this call rather than spending ten seconds serially for every proxy.
    for proxy in proxies.iter().take(16).cloned() {
        tasks.spawn(async move {
            let now = Instant::now();
            let target = std::net::SocketAddr::from(([149, 154, 167, 50], 443));
            let result = crate::proxy::probe(&proxy, target, 2).await;
            let reachable = result.is_ok();
            (proxy, reachable, now.elapsed().as_millis() as u64)
        });
    }
    while let Some(task) = tasks.join_next().await {
        let (proxy, reachable, elapsed) = task.context("Proxy probe task stopped")?;
        let id = proxy.fingerprint()?;
        let result = if reachable {
            "telegram_transport_reachable"
        } else {
            "connection_failed"
        };
        let healthy = json!(reachable);
        state.update_json("proxies-v1.json", json!({}), |store| {
            let sequence = store
                .as_object()
                .context("Proxy pool is invalid")?
                .values()
                .filter_map(|record| record["probe_sequence"].as_u64())
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("Proxy probe sequence exhausted")?;
            if let Some(record) = store.get_mut(&id) {
                record["healthy"] = healthy.clone();
                record["last_result"] = json!(result);
                record["latency_ms"] = json!(elapsed);
                record["probe_sequence"] = json!(sequence);
            }
            Ok(())
        })?;
        let mut row = proxy.describe()?;
        row["result"] = json!(result);
        row["latency_ms"] = json!(elapsed);
        rows.push(row);
    }
    for proxy in proxies.iter().skip(16) {
        let mut row = proxy.describe()?;
        row["result"] = json!("not_tested");
        rows.push(row);
    }
    Ok(rows)
}
