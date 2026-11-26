//! Canonical Telegram permalink building and link-domain host validation.
//!
//! All generated Telegram links (`https://<domain>/<username>`,
//! `https://<domain>/<username>/<post_id>`, `https://<domain>/c/<bare_id>/<post_id>`)
//! and recognized username prefixes share one normalized bare host read from
//! `TELEGRAM_LINK_DOMAIN` (defaulting to `t.me`). URLs are constructed from
//! validated parts via `url::Url`, never by string-pasting untrusted hosts.

use std::fmt;

use url::{Host, Url};

pub const DEFAULT_DOMAIN: &str = "t.me";
pub const LINK_DOMAIN_ENV: &str = "TELEGRAM_LINK_DOMAIN";

/// Errors produced when validating a configured link domain or permalink input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermalinkError {
    InvalidDomain(String),
    InvalidSegment(String),
    InvalidChannelId(i64),
    InvalidMessageId(i64),
}

impl fmt::Display for PermalinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDomain(_) => write!(f, "invalid Telegram link domain"),
            Self::InvalidSegment(_) => write!(f, "invalid Telegram permalink path segment"),
            Self::InvalidChannelId(id) => {
                write!(f, "invalid Telegram channel ID for permalink: {id}")
            }
            Self::InvalidMessageId(id) => {
                write!(f, "invalid Telegram message ID for permalink: {id}")
            }
        }
    }
}

impl std::error::Error for PermalinkError {}

/// Target metadata used to build a canonical message permalink when one exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePermalinkTarget<'a> {
    pub message_id: i64,
    pub username: Option<&'a str>,
    pub chat_id: Option<i64>,
    pub is_channel_or_supergroup: bool,
}

fn is_valid_dns_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || !host.contains('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Validates and normalizes `TELEGRAM_LINK_DOMAIN` to a lowercase bare host.
///
/// Empty or omitted input returns `Ok("t.me")`. Strips an optional `http://` or
/// `https://` scheme and surrounding single root slash, while rejecting userinfo,
/// non-root paths, queries, fragments, ports, whitespace, non-HTTP(S) schemes,
/// and malformed hostnames.
pub fn parse_domain(raw: Option<&str>) -> Result<String, PermalinkError> {
    let trimmed = raw.unwrap_or("").trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_DOMAIN.to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(PermalinkError::InvalidDomain(trimmed.to_string()));
    }

    let (without_scheme, had_scheme) = if let Some(head) = trimmed.get(..8)
        && head.eq_ignore_ascii_case("https://")
    {
        (&trimmed[8..], true)
    } else if let Some(head) = trimmed.get(..7)
        && head.eq_ignore_ascii_case("http://")
    {
        (&trimmed[7..], true)
    } else {
        (trimmed, false)
    };

    let candidate = if had_scheme {
        without_scheme.strip_suffix('/').unwrap_or(without_scheme)
    } else {
        let stripped = without_scheme.strip_prefix('/').unwrap_or(without_scheme);
        stripped.strip_suffix('/').unwrap_or(stripped)
    };

    if candidate.is_empty()
        || candidate
            .chars()
            .any(|c| matches!(c, '/' | '?' | '#' | '@' | '\\' | ':'))
    {
        return Err(PermalinkError::InvalidDomain(trimmed.to_string()));
    }

    let parsed = Url::parse(&format!("https://{candidate}"))
        .map_err(|_| PermalinkError::InvalidDomain(trimmed.to_string()))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_some()
    {
        return Err(PermalinkError::InvalidDomain(trimmed.to_string()));
    }

    match parsed.host() {
        Some(Host::Domain(domain)) if is_valid_dns_host(domain) => Ok(domain.to_string()),
        _ => Err(PermalinkError::InvalidDomain(trimmed.to_string())),
    }
}

/// Normalizes a link domain, falling back to `t.me` if empty or malformed.
pub fn normalize_domain(raw: Option<&str>) -> String {
    parse_domain(raw).unwrap_or_else(|_| DEFAULT_DOMAIN.to_string())
}

/// Reads and strictly validates `TELEGRAM_LINK_DOMAIN` from the environment.
pub fn configured_domain() -> Result<String, PermalinkError> {
    let raw = std::env::var(LINK_DOMAIN_ENV).ok();
    parse_domain(raw.as_deref())
}

/// Reads `TELEGRAM_LINK_DOMAIN` from the environment, falling back to `t.me`.
pub fn link_domain() -> String {
    let raw = std::env::var(LINK_DOMAIN_ENV).ok();
    normalize_domain(raw.as_deref())
}

fn resolve_domain(domain: Option<&str>) -> Result<String, PermalinkError> {
    match domain {
        Some(raw) => parse_domain(Some(raw)),
        None => Ok(link_domain()),
    }
}

/// Builds `https://<domain>/<seg1>/<seg2>...` from validated path segments.
pub fn public_path_link(segments: &[&str], domain: Option<&str>) -> Result<String, PermalinkError> {
    if segments.is_empty() {
        return Err(PermalinkError::InvalidSegment(String::new()));
    }
    let host = resolve_domain(domain)?;
    let mut url = Url::parse(&format!("https://{host}"))
        .map_err(|_| PermalinkError::InvalidDomain(host.clone()))?;
    {
        let mut path_segments = url
            .path_segments_mut()
            .map_err(|_| PermalinkError::InvalidDomain(host))?;
        for &segment in segments {
            let clean = segment.trim();
            if clean.is_empty()
                || clean.chars().any(|c| {
                    matches!(c, '/' | '\\' | '?' | '#') || c.is_whitespace() || c.is_control()
                })
            {
                return Err(PermalinkError::InvalidSegment(segment.to_string()));
            }
            path_segments.push(clean);
        }
    }
    Ok(url.into())
}

/// Builds `https://<domain>/<username>` after trimming optional leading `@`.
pub fn public_link(username: &str, domain: Option<&str>) -> Result<String, PermalinkError> {
    let clean = username.trim().trim_start_matches('@');
    public_path_link(&[clean], domain)
}

/// Extracts the bare channel ID required for `/c/<bare_id>/<post_id>` permalinks.
///
/// Strips the `-100` prefix from marked negative channel IDs without modulo
/// truncation across 64-bit peer IDs; already-bare positive IDs (including those
/// beginning with `100`) are returned unchanged.
pub fn channel_link_id(chat_id: i64) -> u64 {
    let abs = chat_id.unsigned_abs();
    if chat_id >= 0 {
        return abs;
    }
    let text = abs.to_string();
    if let Some(stripped) = text.strip_prefix("100")
        && !stripped.is_empty()
        && let Ok(bare) = stripped.parse::<u64>()
    {
        return bare;
    }
    abs
}

/// Builds a private channel/supergroup permalink `https://<domain>/c/<bare_id>/<message_id>`.
pub fn post_link(
    channel_id: i64,
    message_id: i64,
    domain: Option<&str>,
) -> Result<String, PermalinkError> {
    if message_id <= 0 {
        return Err(PermalinkError::InvalidMessageId(message_id));
    }
    let bare = channel_link_id(channel_id);
    if bare == 0 {
        return Err(PermalinkError::InvalidChannelId(channel_id));
    }
    let bare_str = bare.to_string();
    let msg_str = message_id.to_string();
    public_path_link(&["c", &bare_str, &msg_str], domain)
}

/// Builds a public post permalink `https://<domain>/<username>/<message_id>`.
pub fn public_post_link(
    username: &str,
    message_id: i64,
    domain: Option<&str>,
) -> Result<String, PermalinkError> {
    if message_id <= 0 {
        return Err(PermalinkError::InvalidMessageId(message_id));
    }
    let clean = username.trim().trim_start_matches('@');
    let msg_str = message_id.to_string();
    public_path_link(&[clean, &msg_str], domain)
}

/// Builds the canonical message permalink when the chat supports one:
/// public `https://<domain>/<username>/<id>`, or private channel/supergroup
/// `https://<domain>/c/<bare_id>/<id>`. Returns `None` for DMs or basic groups
/// without a public username.
pub fn message_permalink(
    target: MessagePermalinkTarget<'_>,
    domain: Option<&str>,
) -> Option<String> {
    if target.message_id <= 0 {
        return None;
    }
    if let Some(username) = target.username
        && !username.trim().trim_start_matches('@').is_empty()
    {
        return public_post_link(username, target.message_id, domain).ok();
    }
    if !target.is_channel_or_supergroup {
        return None;
    }
    let chat_id = target.chat_id?;
    post_link(chat_id, target.message_id, domain).ok()
}

/// Returns every recognized link prefix for usernames: built-in `t.me` plus the
/// normalized configured domain (deduplicated), with `https://`, `http://`, and
/// bare-host forms.
pub fn username_prefixes(domain: Option<&str>) -> Vec<String> {
    let active = match domain {
        Some(raw) => normalize_domain(Some(raw)),
        None => link_domain(),
    };
    let mut hosts = vec![DEFAULT_DOMAIN.to_string()];
    if active != DEFAULT_DOMAIN {
        hosts.push(active);
    }
    let mut prefixes = Vec::with_capacity(hosts.len() * 3);
    for host in hosts {
        prefixes.push(format!("https://{host}/"));
        prefixes.push(format!("http://{host}/"));
        prefixes.push(format!("{host}/"));
    }
    prefixes
}

/// Returns `true` only when `host` strictly matches `t.me` or the active domain.
pub fn is_recognized_link_host(host: &str, domain: Option<&str>) -> bool {
    let Ok(parsed_host) = parse_domain(Some(host)) else {
        return false;
    };
    let active = match domain {
        Some(raw) => normalize_domain(Some(raw)),
        None => link_domain(),
    };
    parsed_host == DEFAULT_DOMAIN || parsed_host == active
}

/// Strips a recognized built-in (`t.me`) or configured-domain prefix or leading
/// `@` from a username input, validating the host boundary exactly.
pub fn strip_username_prefix(raw: &str, domain: Option<&str>) -> String {
    let trimmed = raw.trim();
    for prefix in username_prefixes(domain) {
        if let Some(head) = trimmed.get(..prefix.len())
            && head.eq_ignore_ascii_case(&prefix)
        {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed
        .strip_prefix('@')
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_normalize_domain_accept_valid_and_reject_malformed_hosts() {
        assert_eq!(parse_domain(None), Ok("t.me".to_string()));
        assert_eq!(parse_domain(Some("")), Ok("t.me".to_string()));
        assert_eq!(parse_domain(Some("   ")), Ok("t.me".to_string()));

        for valid in [
            "tg.example",
            "TG.EXAMPLE",
            "https://tg.example",
            "https://tg.example/",
            "http://TG.EXAMPLE/",
            " tg.example/ ",
            "/tg.example/",
        ] {
            assert_eq!(
                parse_domain(Some(valid)),
                Ok("tg.example".to_string()),
                "{valid}"
            );
            assert_eq!(normalize_domain(Some(valid)), "tg.example");
        }

        for invalid in [
            "https://user@tg.example",
            "user:pass@tg.example",
            "https://tg.example/path",
            "tg.example/c/123",
            "https://tg.example?q=1",
            "https://tg.example#frag",
            "ftp://tg.example",
            "javascript:alert(1)",
            "https://",
            "/",
            "tg .example",
            "tg..example",
            "-tg.example",
            "tg_example.com",
            "tg.example:8443",
            "https:///tg.example",
            "localhost",
        ] {
            assert!(
                parse_domain(Some(invalid)).is_err(),
                "expected error for {invalid:?}"
            );
            assert_eq!(normalize_domain(Some(invalid)), "t.me");
        }
    }

    #[test]
    fn channel_link_id_strips_marked_prefix_only_and_preserves_64bit_ids() {
        assert_eq!(channel_link_id(-1001234567890), 1234567890);
        assert_eq!(channel_link_id(-10012345678901234), 12345678901234);
        assert_eq!(channel_link_id(1001234567), 1001234567);
        assert_eq!(channel_link_id(42), 42);
        assert_eq!(channel_link_id(-100), 100);
        assert_eq!(channel_link_id(-999888), 999888);
        assert_eq!(channel_link_id(i64::MIN), 9_223_372_036_854_775_808_u64);
    }

    #[test]
    fn builds_public_private_and_message_permalinks() {
        assert_eq!(
            public_link("news_room", Some("t.me")),
            Ok("https://t.me/news_room".into())
        );
        assert_eq!(
            public_link("@news_room", Some("tg.example")),
            Ok("https://tg.example/news_room".into())
        );
        assert_eq!(
            public_path_link(&["addstickers", "HotCherry"], Some("t.me")),
            Ok("https://t.me/addstickers/HotCherry".into())
        );
        assert_eq!(
            post_link(-1001234567890, 77, Some("t.me")),
            Ok("https://t.me/c/1234567890/77".into())
        );
        assert_eq!(
            post_link(1001234567, 77, Some("tg.example")),
            Ok("https://tg.example/c/1001234567/77".into())
        );
        assert!(post_link(-100123, 0, Some("t.me")).is_err());
        assert!(public_link("bad/seg", Some("t.me")).is_err());

        let pub_target = MessagePermalinkTarget {
            message_id: 7,
            username: Some("news"),
            chat_id: Some(-10099),
            is_channel_or_supergroup: true,
        };
        assert_eq!(
            message_permalink(pub_target, Some("tg.example")),
            Some("https://tg.example/news/7".into())
        );

        let priv_target = MessagePermalinkTarget {
            message_id: 9,
            username: None,
            chat_id: Some(-10055555),
            is_channel_or_supergroup: true,
        };
        assert_eq!(
            message_permalink(priv_target, Some("t.me")),
            Some("https://t.me/c/55555/9".into())
        );

        let dm_target = MessagePermalinkTarget {
            message_id: 9,
            username: None,
            chat_id: Some(12345),
            is_channel_or_supergroup: false,
        };
        assert_eq!(message_permalink(dm_target, Some("t.me")), None);
    }

    #[test]
    fn username_prefix_recognition_validates_host_exactly() {
        for raw in [
            "@news_room",
            "t.me/news_room",
            "https://t.me/news_room",
            "http://T.ME/news_room",
            "news_room",
        ] {
            assert_eq!(
                strip_username_prefix(raw, Some("t.me")),
                "news_room",
                "{raw}"
            );
        }
        assert_eq!(
            strip_username_prefix("https://tg.example/news_room", Some("tg.example")),
            "news_room"
        );
        assert_eq!(
            strip_username_prefix("t.me/news_room", Some("tg.example")),
            "news_room"
        );
        assert_eq!(strip_username_prefix("tmember", Some("t.me")), "tmember");

        // Spoofed/foreign hosts must never be stripped
        assert_eq!(
            strip_username_prefix("https://t.me.evil.com/news_room", Some("t.me")),
            "https://t.me.evil.com/news_room"
        );
        assert_eq!(
            strip_username_prefix("https://t.me@evil.com/news_room", Some("t.me")),
            "https://t.me@evil.com/news_room"
        );
        assert_eq!(
            strip_username_prefix("https://other.example/news_room", Some("tg.example")),
            "https://other.example/news_room"
        );
        assert!(is_recognized_link_host("T.ME", Some("tg.example")));
        assert!(is_recognized_link_host("tg.example", Some("tg.example")));
        assert!(!is_recognized_link_host(
            "t.me.evil.com",
            Some("tg.example")
        ));
        assert_eq!(username_prefixes(Some("t.me")).len(), 3);
        assert_eq!(username_prefixes(Some("tg.example")).len(), 6);
    }
}
