//! Read-only Telegram source selectors. Links select an existing peer; they
//! never join an invite, open a web URL, or execute bot/deep-link actions.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn source_selector(value: &Value) -> Result<Value> {
    if let Some(id) = value.as_i64() {
        ensure!(id != 0, "A proxy source id must not be zero");
        return Ok(json!(id.to_string()));
    }
    let text = value
        .as_str()
        .context("A proxy source must be a Telegram selector or link")?
        .trim();
    ensure!(
        !text.is_empty() && text.len() <= 2048 && !text.chars().any(char::is_control),
        "A proxy source is empty, oversized or contains control characters"
    );
    if let Ok(id) = text.parse::<i64>() {
        ensure!(id != 0, "A proxy source id must not be zero");
        return Ok(json!(id.to_string()));
    }
    if text
        .strip_prefix('-')
        .unwrap_or(text)
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        bail!("The proxy source id exceeds the signed 64-bit range");
    }
    let lower = text.to_ascii_lowercase();
    let bare_link = [
        "t.me/",
        "telegram.me/",
        "telegram.dog/",
        "www.t.me/",
        "www.telegram.me/",
        "www.telegram.dog/",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
        || lower
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|host| host.ends_with(".t.me"));
    if !text.contains("://") && !lower.starts_with("tg:") && !bare_link {
        return if let Some(name) = text.strip_prefix('@') {
            username(name)
        } else {
            Ok(json!(text))
        };
    }
    let candidate = if bare_link {
        format!("https://{text}")
    } else {
        text.to_owned()
    };
    let url = url::Url::parse(&candidate).context("Invalid Telegram proxy-source link")?;
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.port().is_none(),
        "Telegram source links must not contain credentials or nonstandard ports"
    );
    if url.scheme() == "tg" {
        let route = if let Some(host) = url.host_str() {
            ensure!(
                url.path().trim_matches('/').is_empty(),
                "Unsupported Telegram deep-link path"
            );
            host
        } else {
            url.path().trim_matches('/')
        };
        let query = parameters(&url)?;
        return match route.to_ascii_lowercase().as_str() {
            "resolve" => username(
                query
                    .get("domain")
                    .context("Telegram source link has no username")?,
            ),
            "privatepost" => {
                positive_message(
                    query
                        .get("post")
                        .context("A private message source needs its post id")?,
                )?;
                private_channel(
                    query
                        .get("channel")
                        .context("Private source link has no channel id")?,
                )
            }
            _ => bail!("This Telegram deep link does not identify a readable source peer"),
        };
    }
    let host = url.host_str().unwrap_or("");
    let standard_host = matches!(
        host,
        "t.me"
            | "telegram.me"
            | "telegram.dog"
            | "www.t.me"
            | "www.telegram.me"
            | "www.telegram.dog"
    );
    let subdomain = if standard_host {
        None
    } else {
        host.strip_suffix(".t.me")
    };
    ensure!(
        matches!(url.scheme(), "https" | "http") && (standard_host || subdomain.is_some()),
        "Proxy source links must use an official Telegram host"
    );
    let path = url.path().trim_matches('/');
    let mut segments = if path.is_empty() {
        vec![]
    } else {
        path.split('/')
            .map(super::decode_component)
            .collect::<Result<Vec<_>>>()?
    };
    // Official link semantics ignore fragments and treat <username>.t.me as
    // t.me/<username>. They never change the whole-peer scope of this import.
    if let Some(name) = subdomain {
        ensure!(
            name.len() > 1 && name != "www" && !action_name(name),
            "This Telegram subdomain does not name a source peer"
        );
        username(name)?;
        segments.insert(0, name.to_owned());
    }
    ensure!(!segments.is_empty(), "Telegram source link has no peer");
    match segments[0].as_str() {
        "c" => {
            ensure!(
                (3..=4).contains(&segments.len()),
                "A private Telegram source must be a message link"
            );
            for id in &segments[2..] {
                positive_message(id)?;
            }
            private_channel(&segments[1])
        }
        "s" => {
            ensure!(
                (2..=4).contains(&segments.len()),
                "Invalid Telegram source preview link"
            );
            for id in &segments[2..] {
                positive_message(id)?;
            }
            username(&segments[1])
        }
        name if action_name(name) => {
            bail!("This Telegram link is an action or invite rather than a readable source peer")
        }
        name => {
            ensure!(segments.len() <= 3, "Invalid Telegram source message link");
            for id in &segments[1..] {
                positive_message(id)?;
            }
            username(name)
        }
    }
}
fn action_name(name: &str) -> bool {
    matches!(
        name,
        "joinchat"
            | "proxy"
            | "socks"
            | "share"
            | "iv"
            | "login"
            | "confirmphone"
            | "setlanguage"
            | "addstickers"
            | "addemoji"
            | "addlist"
            | "addstyle"
            | "addtheme"
            | "auction"
            | "auth"
            | "bg"
            | "boost"
            | "call"
            | "invoice"
            | "giftcode"
            | "m"
            | "contact"
            | "nft"
            | "oauth"
            | "web"
    )
}
fn username(name: &str) -> Result<Value> {
    ensure!(
        (1..=64).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "Telegram source username is invalid"
    );
    Ok(json!(format!("@{}", name.to_ascii_lowercase())))
}
fn positive_message(value: &str) -> Result<()> {
    ensure!(
        value.parse::<i32>().is_ok_and(|id| id > 0),
        "Telegram source message/thread ids must be positive 32-bit integers"
    );
    Ok(())
}
fn private_channel(value: &str) -> Result<Value> {
    let id = value
        .parse::<i64>()
        .context("Private Telegram source channel id is invalid")?;
    ensure!(
        id > 0,
        "Private Telegram source channel id must be positive"
    );
    let marked = (-1_000_000_000_000i64)
        .checked_sub(id)
        .context("Private source channel id exceeds its supported range")?;
    Ok(json!(marked.to_string()))
}
fn parameters(url: &url::Url) -> Result<BTreeMap<String, String>> {
    let mut params = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        ensure!(
            params
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Telegram source link has repeated query parameters"
        );
    }
    Ok(params)
}
