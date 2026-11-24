//! UTF-16-aware text and a script-free HTML export renderer.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn html_escape(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        match ch {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#39;"),
            '\n' | '\u{2028}' | '\u{2029}' => result.push_str("<br>"),
            ch if ch.is_control() => result.push_str(&format!("&#x{:02X};", ch as u32)),
            _ => result.push(ch),
        }
    }
    result
}

pub fn utf16_byte_offset(text: &str, offset: usize) -> Result<usize> {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units == offset {
            return Ok(index);
        }
        units += ch.len_utf16();
        if units > offset {
            bail!("Text entity offset splits a UTF-16 surrogate pair");
        }
    }
    if units == offset {
        Ok(text.len())
    } else {
        bail!("Text entity offset is outside the message")
    }
}

pub fn safe_href(value: &str) -> Option<String> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(|c| {
            c.is_control()
                || c.is_whitespace()
                || matches!(c, '\\' | '<' | '>' | '"' | '\'' | '{' | '}')
        })
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if ["%00", "%0a", "%0d", "%09", "%1b"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return None;
    }
    let with_scheme = if !value.contains(':') && !value.starts_with('/') {
        format!("https://{value}")
    } else {
        value.to_owned()
    };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    match parsed.scheme() {
        "http" | "https" if parsed.host_str().is_some() => Some(parsed.to_string()),
        "tg" if parsed.host_str().is_some() => Some(parsed.to_string()),
        "mailto"
            if parsed.path().contains('@')
                && !parsed.path().contains("@@")
                && parsed.query().is_none() =>
        {
            Some(parsed.to_string())
        }
        "tel"
            if parsed.path().chars().any(|c| c.is_ascii_digit())
                && parsed
                    .path()
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | '(' | ')')) =>
        {
            Some(parsed.to_string())
        }
        _ => None,
    }
}

fn integer(value: &Value, key: &str) -> Result<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| anyhow::anyhow!("Text entity {key} must be a nonnegative integer"))
}

pub fn relative_href(value: &str) -> Option<String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', ':'])
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    let mut url = url::Url::parse("https://local.invalid/").ok()?;
    {
        let mut segments = url.path_segments_mut().ok()?;
        for part in value.split('/') {
            segments.push(part);
        }
    }
    Some(url.path().trim_start_matches('/').to_owned())
}

fn tags(entity: &Value, text: &str, files: &Value) -> (String, &'static str) {
    match entity["_"].as_str().unwrap_or_default() {
        "messageEntityBold" => ("<strong>".into(), "</strong>"),
        "messageEntityItalic" => ("<em>".into(), "</em>"),
        "messageEntityUnderline" => ("<u>".into(), "</u>"),
        "messageEntityStrike" => ("<s>".into(), "</s>"),
        "messageEntityCode" => ("<code>".into(), "</code>"),
        "messageEntityPre" => ("<pre>".into(), "</pre>"),
        "messageEntityBlockquote" => ("<blockquote>".into(), "</blockquote>"),
        "messageEntitySpoiler" => ("<details><summary>Spoiler</summary>".into(), "</details>"),
        "messageEntityTextUrl" | "messageEntityUrl" => {
            let target = entity.get("url").and_then(Value::as_str).unwrap_or(text);
            match safe_href(target) {
                Some(href) => (
                    format!(
                        "<a rel=\"noreferrer noopener\" href=\"{}\">",
                        html_escape(&href)
                    ),
                    "</a>",
                ),
                None => (String::new(), ""),
            }
        }
        "messageEntityEmail" => match safe_href(&format!("mailto:{text}")) {
            Some(href) => (format!("<a href=\"{}\">", html_escape(&href)), "</a>"),
            None => (String::new(), ""),
        },
        "messageEntityCustomEmoji" => {
            let id = entity["document_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| entity["document_id"].to_string());
            match files
                .get(&id)
                .and_then(Value::as_str)
                .and_then(relative_href)
            {
                Some(path) => (
                    format!("<a class=\"custom-emoji\" href=\"{}\">", html_escape(&path)),
                    "</a>",
                ),
                None => (String::new(), ""),
            }
        }
        "messageEntityMention"
            if text.strip_prefix('@').is_some_and(|s| {
                !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }) =>
        {
            (
                format!("<a href=\"https://t.me/{}\">", html_escape(&text[1..])),
                "</a>",
            )
        }
        _ => (String::new(), ""),
    }
}

/// Reopen active entities at segment boundaries so crossing ranges produce
/// valid HTML, while all original ranges are preserved in the JSON export.
pub fn render_entities(text: &str, entities: &[Value]) -> Result<String> {
    render_entities_with_files(text, entities, &Value::Null)
}

pub fn render_entities_with_files(text: &str, entities: &[Value], files: &Value) -> Result<String> {
    let mut ranges = Vec::with_capacity(entities.len());
    let mut boundaries = BTreeMap::from([(0, ()), (text.len(), ())]);
    for entity in entities {
        let offset = integer(entity, "offset")?;
        let end = offset
            .checked_add(integer(entity, "length")?)
            .ok_or_else(|| anyhow::anyhow!("Text entity range overflows"))?;
        let (start, end) = (
            utf16_byte_offset(text, offset)?,
            utf16_byte_offset(text, end)?,
        );
        if start == end {
            continue;
        }
        boundaries.insert(start, ());
        boundaries.insert(end, ());
        ranges.push((start, end, entity));
    }
    ranges.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let positions: Vec<usize> = boundaries.keys().copied().collect();
    let mut output = String::new();
    for pair in positions.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let mut closes = Vec::new();
        for (from, until, entity) in &ranges {
            if *from <= start && *until >= end {
                let (open, close) = tags(entity, &text[*from..*until], files);
                output.push_str(&open);
                closes.push(close);
            }
        }
        output.push_str(&html_escape(&text[start..end]));
        for close in closes.into_iter().rev() {
            output.push_str(close);
        }
    }
    Ok(output)
}

pub fn person_name(user: &Value) -> String {
    let name = [user["first_name"].as_str(), user["last_name"].as_str()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !name.is_empty() {
        name
    } else if user["deleted"] == true {
        "Deleted Account".into()
    } else {
        user["username"].as_str().unwrap_or("Unknown user").into()
    }
}

pub fn json_message(message: &Value, peers: &BTreeMap<String, Value>) -> Result<Value> {
    let date = message.get("date").and_then(Value::as_i64).unwrap_or(0);
    let date_text = chrono::DateTime::from_timestamp(date, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default();
    let from_id = message["from_id"]
        .get("user_id")
        .or_else(|| message["from_id"].get("channel_id"))
        .or_else(|| message["from_id"].get("chat_id"));
    let from_id = from_id.map(|v| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    });
    let namespace = if message["from_id"].get("user_id").is_some() {
        "user"
    } else if message["from_id"].get("channel_id").is_some() {
        "channel"
    } else {
        "chat"
    };
    let sender = from_id.as_ref().and_then(|id| {
        peers
            .get(&format!("{namespace}:{id}"))
            .or_else(|| peers.get(id))
    });
    let mut value = json!({"id":message["id"],"type":if message["_"].as_str()==Some("messageService") {"service"}else{"message"},
        "date":date_text,"date_unixtime":date.to_string(),"text":message["message"].as_str().unwrap_or_default(),
        "text_entities":message.get("entities").cloned().unwrap_or_else(||json!([]))});
    if let Some(id) = from_id {
        value["from_id"] = format!("{namespace}{id}").into();
    }
    if let Some(sender) = sender {
        value["from"] = sender
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| person_name(sender))
            .into();
    }
    for (raw, key) in [
        ("reply_to", "reply_to"),
        ("fwd_from", "forwarded_from"),
        ("reactions", "reactions"),
        ("media", "media"),
        ("action", "action"),
        ("reply_markup", "inline_bot_buttons"),
        ("rich_message", "rich_message"),
    ] {
        if let Some(data) = message.get(raw).filter(|v| !v.is_null()) {
            value[key] = data.clone();
        }
    }
    if let Some(path) = message.get("export_file") {
        value[if message["media"].get("photo").is_some() {
            "photo"
        } else {
            "file"
        }] = path.clone();
    }
    for key in [
        "export_file_skipped",
        "rich_files",
        "source_message_id",
        "source_peer",
        "custom_emoji_files",
        "custom_emoji_unavailable",
    ] {
        if let Some(data) = message.get(key) {
            value[key] = data.clone();
        }
    }
    // The original wire object retains new layer fields while the structured
    // writer is extended; this is source data, never executable markup.
    value["telegram_tl"] = message.clone();
    Ok(value)
}

pub fn html_message(message: &Value, peers: &BTreeMap<String, Value>) -> Result<String> {
    let record = json_message(message, peers)?;
    let id = message["id"]
        .as_i64()
        .map(|v| v.to_string())
        .or_else(|| message["id"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let text = message["message"].as_str().unwrap_or_default();
    let empty = Vec::new();
    let body = render_entities_with_files(
        text,
        message["entities"].as_array().unwrap_or(&empty),
        &message["custom_emoji_files"],
    )?;
    let mut html = format!(
        "<article class=\"message\" id=\"message{}\"><header><b>{}</b><time>{}</time></header><div class=\"text\">{}</div>",
        html_escape(&id),
        html_escape(record["from"].as_str().unwrap_or("")),
        html_escape(record["date"].as_str().unwrap_or("")),
        body
    );
    if let Some(action) = message.get("action").filter(|a| !a.is_null()) {
        html.push_str(&format!(
            "<pre class=\"service\">{}</pre>",
            html_escape(&serde_json::to_string_pretty(action)?)
        ));
    }
    if let Some(rich) = message.get("rich_message").filter(|value| !value.is_null()) {
        html.push_str(&super::rich::message(rich, &message["rich_files"])?);
    }
    if let Some(media) = message.get("export_file").and_then(Value::as_str)
        && let Some(href) = relative_href(media)
    {
        html.push_str(&format!(
            "<a class=\"media\" href=\"{}\">{}</a>",
            html_escape(&href),
            html_escape(media)
        ));
    }
    html.push_str("</article>\n");
    Ok(html)
}

pub fn html_header(title: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; media-src 'self'\"><title>{}</title><style>body{{font:16px system-ui;max-width:900px;margin:auto;background:#eef1f5;color:#17212b}}h1{{padding:1rem}}article{{padding:1rem;margin:1rem;background:white;border-radius:8px;overflow-wrap:anywhere}}header{{display:flex;justify-content:space-between;gap:1rem}}time{{font-size:12px;color:#555}}.text{{padding-top:.6rem;white-space:normal}}pre{{white-space:pre-wrap}}blockquote{{border-inline-start:3px solid #4a8bbf;padding-inline-start:1rem}}a{{color:#1675b6}}</style><h1>{}</h1>\n",
        html_escape(title),
        html_escape(title)
    )
}
