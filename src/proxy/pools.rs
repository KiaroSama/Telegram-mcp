use super::*;
use anyhow::{bail, ensure};
use std::collections::BTreeMap;

fn account_setting<'a>(snapshot: &'a ConfigSnapshot, field: &str, label: &str) -> Option<&'a str> {
    snapshot
        .values
        .get(&format!(
            "TELEGRAM_PROXY_{field}_{}",
            label.to_ascii_uppercase()
        ))
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim())
}

fn global_setting<'a>(snapshot: &'a ConfigSnapshot, field: &str) -> Option<&'a str> {
    snapshot
        .values
        .get(&format!("TELEGRAM_PROXY_{field}"))
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim())
}

fn setting<'a>(snapshot: &'a ConfigSnapshot, field: &str, label: &str) -> Option<&'a str> {
    account_setting(snapshot, field, label).or_else(|| global_setting(snapshot, field))
}

fn parse_rdns(raw: Option<&str>) -> Result<bool> {
    let Some(value) = raw else {
        return Ok(true);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("TELEGRAM_PROXY_RDNS must be a boolean value"),
    }
}

pub(super) fn validate(proxy: &Proxy) -> Result<()> {
    ensure!(
        !proxy.host.is_empty()
            && proxy.host.len() <= 253
            && !proxy
                .host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            && proxy.port > 0,
        "Invalid proxy host or port"
    );
    ensure!(
        matches!(
            proxy.kind.as_str(),
            "socks5" | "socks4" | "http" | "https" | "mtproto"
        ),
        "Configured proxy protocol is unsupported"
    );
    for credential in [&proxy.username, &proxy.password].into_iter().flatten() {
        ensure!(
            credential.len() <= 255 && !credential.chars().any(|c| c.is_control()),
            "Proxy credentials exceed their protocol bounds"
        );
    }
    if proxy.kind == "socks4" {
        ensure!(
            proxy.password.as_ref().is_none_or(|p| p.is_empty()),
            "SOCKS4 does not support password authentication"
        );
    }
    if proxy.kind == "mtproto" {
        ensure!(
            proxy.username.as_ref().is_none_or(|u| u.is_empty())
                && proxy.password.as_ref().is_none_or(|p| p.is_empty()),
            "MTProto proxy does not support username or password authentication"
        );
        mtproxy::Secret::parse(
            proxy
                .secret
                .as_deref()
                .context("MTProto proxy secret is missing")?,
        )?;
    }
    Ok(())
}

fn parse_typed(snapshot: &ConfigSnapshot, label: &str) -> Result<Proxy> {
    let kind = setting(snapshot, "TYPE", label)
        .context("Proxy settings need TELEGRAM_PROXY_TYPE or TELEGRAM_PROXY_URL")?;
    let kind_lower = kind.to_ascii_lowercase();
    let kind = match kind_lower.as_str() {
        "socks5" | "socks5h" => "socks5",
        "socks4" | "socks4a" => "socks4",
        "http" => "http",
        "https" => "https",
        "mtproto" | "mtproxy" => "mtproto",
        _ => bail!("Configured proxy protocol is unsupported"),
    };
    let rdns = parse_rdns(setting(snapshot, "RDNS", label))?;
    if !rdns && matches!(kind_lower.as_str(), "socks5h" | "socks4a") {
        bail!("TELEGRAM_PROXY_RDNS=false conflicts with remote-DNS proxy scheme");
    }
    let port: u16 = setting(snapshot, "PORT", label)
        .context("Configured proxy port is missing")?
        .parse()
        .context("Configured proxy port is invalid")?;
    ensure!(port >= 1, "Configured proxy port must be in 1..=65535");
    let proxy = Proxy {
        kind: kind.into(),
        host: setting(snapshot, "HOST", label)
            .context("Configured proxy host is missing")?
            .trim_matches(['[', ']'])
            .into(),
        port,
        secret: setting(snapshot, "SECRET", label).map(str::to_owned),
        username: setting(snapshot, "USERNAME", label).map(str::to_owned),
        password: setting(snapshot, "PASSWORD", label).map(str::to_owned),
    };
    validate(&proxy)?;
    Ok(proxy)
}

fn configured(snapshot: &ConfigSnapshot, label: &str) -> Result<Option<Proxy>> {
    const TYPED_FIELDS: &[&str] = &[
        "TYPE", "HOST", "PORT", "SECRET", "RDNS", "USERNAME", "PASSWORD",
    ];
    let account_url = account_setting(snapshot, "URL", label);
    let account_typed = TYPED_FIELDS
        .iter()
        .any(|field| account_setting(snapshot, field, label).is_some());
    if account_url.is_some() && account_typed {
        bail!(
            "Conflicting account proxy settings: specify either TELEGRAM_PROXY_URL or typed fields"
        );
    }
    if let Some(url) = account_url {
        let proxy = crate::tools::local::proxies::parse_proxy(url)
            .context("Configured proxy URL is invalid")?;
        validate(&proxy)?;
        return Ok(Some(proxy));
    }
    if account_typed {
        return Ok(Some(parse_typed(snapshot, label)?));
    }
    let global_url = global_setting(snapshot, "URL");
    let global_typed = TYPED_FIELDS
        .iter()
        .any(|field| global_setting(snapshot, field).is_some());
    if global_url.is_some() && global_typed {
        bail!(
            "Conflicting global proxy settings: specify either TELEGRAM_PROXY_URL or typed fields"
        );
    }
    if let Some(url) = global_url {
        let proxy = crate::tools::local::proxies::parse_proxy(url)
            .context("Configured proxy URL is invalid")?;
        validate(&proxy)?;
        return Ok(Some(proxy));
    }
    if global_typed {
        return Ok(Some(parse_typed(snapshot, label)?));
    }
    Ok(None)
}

pub(super) fn candidates(
    state: &State,
    snapshot: &ConfigSnapshot,
    label: &str,
) -> Result<Vec<Candidate>> {
    import_legacy(state)?;
    let mut choices = vec![];
    if let Some(proxy) = configured(snapshot, label)? {
        choices.push(Candidate {
            id: "env".into(),
            proxy: Some(proxy),
        });
    }
    choices.push(Candidate {
        id: "direct".into(),
        proxy: None,
    });
    let store = state.read_json("proxies-v1.json", json!({}))?;
    let rows = store.as_object().context("Proxy pool state is invalid")?;
    ensure!(
        rows.len() <= 4096,
        "Proxy pool exceeds its 4096-entry bound"
    );
    let own = rows.values().any(|record| {
        in_pool(record, label)
            && serde_json::from_value::<Proxy>(record["proxy"].clone())
                .is_ok_and(|proxy| validate(&proxy).is_ok())
    });
    let selected = if own { label } else { "shared" };
    let mut pooled = vec![];
    for (id, record) in rows {
        if !in_pool(record, selected) {
            continue;
        }
        let Ok(proxy) = serde_json::from_value::<Proxy>(record["proxy"].clone()) else {
            continue;
        };
        if validate(&proxy).is_err() {
            continue;
        }
        let health = match record["healthy"].as_bool() {
            Some(true) => 0,
            None => 1,
            Some(false) => 2,
        };
        let latency = record["latency_ms"].as_u64().unwrap_or(u64::MAX);
        pooled.push((
            (health, latency, id.clone()),
            Candidate {
                id: id.clone(),
                proxy: Some(proxy),
            },
        ));
    }
    pooled.sort_by(|a, b| a.0.cmp(&b.0));
    choices.extend(pooled.into_iter().map(|(_, candidate)| candidate));
    Ok(choices)
}

/// Import each legacy pool once. Old bytes are preserved, and newly managed
/// entries win when the same capability already exists in the native store.
fn import_legacy(state: &State) -> Result<()> {
    if state.read_json("proxy-legacy-import-v1.json", json!(false))? == json!(true) {
        return Ok(());
    }
    let legacy = state.read_json("proxy-pool.json", Value::Null)?;
    if legacy.is_null() {
        return Ok(());
    }
    let object = legacy
        .as_object()
        .context("Legacy proxy pool is invalid; it was preserved")?;
    ensure!(
        object
            .keys()
            .all(|key| ["shared", "accounts", "routes"].contains(&key.as_str())),
        "Legacy proxy pool contains unsupported data; it was preserved"
    );
    let mut entries: BTreeMap<String, Value> = BTreeMap::new();
    let mut memberships = 0usize;
    let mut add = |pool: &str, value: &Value| -> Result<()> {
        let rows = value
            .as_array()
            .context("Legacy proxy pool entries must be an array")?;
        for row in rows {
            memberships += 1;
            ensure!(
                memberships <= 65_536,
                "Legacy proxy pool exceeds its membership bound"
            );
            let Ok(proxy) = serde_json::from_value::<Proxy>(row.clone()) else {
                continue;
            };
            if validate(&proxy).is_err() {
                continue;
            }
            let id = proxy.fingerprint()?;
            let pool = pool.to_ascii_lowercase();
            if let Some(existing) = entries.get_mut(&id) {
                let tags = existing["pools"]
                    .as_array_mut()
                    .context("Migrated proxy memberships are invalid")?;
                if !tags.contains(&json!(pool)) {
                    tags.push(json!(pool));
                }
            } else {
                ensure!(
                    entries.len() < 4096,
                    "Legacy proxy pool exceeds 4096 entries"
                );
                entries.insert(id,json!({"proxy":proxy,"pool":pool,"pools":[pool],"healthy":row["healthy"],"last_result":row["last_result"],"latency_ms":row["latency_ms"],"source":"legacy_import"}));
            }
        }
        Ok(())
    };
    if let Some(shared) = object.get("shared") {
        add("shared", shared)?;
    }
    if let Some(accounts) = object.get("accounts") {
        for (label, rows) in accounts
            .as_object()
            .context("Legacy account proxy pools are invalid")?
        {
            crate::session::validate_label(label)?;
            add(label, rows)?;
        }
    }
    state.update_json("proxies-v1.json", json!({}), |store| {
        let rows = store
            .as_object_mut()
            .context("Native proxy pool state is invalid")?;
        for (id, record) in entries {
            if !rows.contains_key(&id) {
                ensure!(rows.len() < 4096, "Native proxy pool reached 4096 entries");
                rows.insert(id, record);
            }
        }
        Ok(())
    })?;
    state.write_json("proxy-legacy-import-v1.json", &json!(true))?;
    Ok(())
}

pub(super) fn in_pool(record: &Value, pool: &str) -> bool {
    record["pool"]
        .as_str()
        .is_some_and(|tag| tag.eq_ignore_ascii_case(pool))
        || record["pools"].as_array().is_some_and(|tags| {
            tags.iter().any(|tag| {
                tag.as_str()
                    .is_some_and(|tag| tag.eq_ignore_ascii_case(pool))
            })
        })
}
