//! Desktop-compatible `messages.html` renderer for typed Recent Actions (`AdminLogEntry`).

use super::admin_log::{AdminLogEntry, FieldChange, MessageSnapshot};
use super::admin_log_text::{AdminLogPart, AdminTextKind, AdminTextPart};
use super::files::clean_relative_path;
use super::json_admin::project_snapshot_text;
use super::render::{html_escape, person_name, relative_href, render_entities, safe_href};
use crate::text_fidelity::{DEFAULT_MAX_NAME_CHARS, display_name};
use serde_json::Value;
use std::collections::BTreeMap;

fn format_iso_utc(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn render_admin_text_parts(parts: &[AdminTextPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part.kind {
            AdminTextKind::Plain => out.push_str(&html_escape(&part.text)),
            AdminTextKind::Bold => {
                out.push_str("<strong>");
                out.push_str(&html_escape(&part.text));
                out.push_str("</strong>");
            }
            AdminTextKind::Italic => {
                out.push_str("<em>");
                out.push_str(&html_escape(&part.text));
                out.push_str("</em>");
            }
            AdminTextKind::Url => match safe_href(&part.text) {
                Some(href) => out.push_str(&format!(
                    "<a rel=\"noreferrer noopener\" href=\"{}\">{}</a>",
                    html_escape(&href),
                    html_escape(&part.text)
                )),
                None => out.push_str(&html_escape(&part.text)),
            },
        }
    }
    out
}

fn render_field_changes(changes: &[FieldChange]) -> String {
    if changes.is_empty() {
        return String::new();
    }
    let mut out = String::from("<ul class=\"field-changes\">");
    for chg in changes {
        let field = html_escape(&chg.field);
        let before = html_escape(chg.before.as_deref().unwrap_or("none"));
        let after = html_escape(chg.after.as_deref().unwrap_or("none"));
        out.push_str(&format!(
            "<li data-field=\"{field}\"><span class=\"field-name\">{field}</span>: <del>{before}</del> &rarr; <ins>{after}</ins></li>"
        ));
    }
    out.push_str("</ul>");
    out
}

fn sender_display(
    snapshot: &MessageSnapshot,
    entry: &AdminLogEntry,
    peers: &BTreeMap<String, Value>,
) -> String {
    if let Some(ref sid) = snapshot.sender_id {
        if let Some(user) = peers.get(&format!("user:{sid}")).or_else(|| peers.get(sid)) {
            return person_name(user);
        }
        if let Some(chan) = peers.get(&format!("channel:{sid}")) {
            let title = chan["title"].as_str().unwrap_or("Channel");
            return display_name(Some(title), DEFAULT_MAX_NAME_CHARS);
        }
    }
    display_name(Some(&entry.actor.name), DEFAULT_MAX_NAME_CHARS)
}

fn render_snapshot_media(snapshot: &MessageSnapshot) -> String {
    let mut out = String::new();
    if let Some(thumb) = snapshot
        .thumbnail_file
        .as_deref()
        .and_then(clean_relative_path)
        .and_then(|p| relative_href(&p).map(|href| (p, href)))
    {
        out.push_str(&format!(
            "<img class=\"thumb\" loading=\"lazy\" alt=\"Thumbnail\" src=\"{}\">",
            html_escape(&thumb.1)
        ));
    }
    if let Some((rel, href)) = snapshot
        .export_file
        .as_deref()
        .and_then(clean_relative_path)
        .and_then(|p| relative_href(&p).map(|href| (p, href)))
    {
        out.push_str(&format!(
            "<a class=\"media\" href=\"{}\">{}</a>",
            html_escape(&href),
            html_escape(&rel)
        ));
    }
    out
}

/// Renders one typed `AdminLogEntry` and its `AdminLogPart` list as safe Desktop HTML articles.
/// Returns `(rendered_html, article_count)` so 1000-entry HTML pagination stays exact.
pub fn render_event_html(
    entry: &AdminLogEntry,
    parts: &[AdminLogPart],
    peers: &BTreeMap<String, Value>,
) -> (String, usize) {
    let mut html = String::new();
    let mut count = 0;
    let event_id = html_escape(&entry.event_id);
    let action_slug = html_escape(entry.action_kind.as_str());
    let actor = html_escape(&display_name(
        Some(&entry.actor.name),
        DEFAULT_MAX_NAME_CHARS,
    ));
    let date_iso = html_escape(&format_iso_utc(entry.timestamp));

    if parts.is_empty() {
        html.push_str(&format!(
            "<article class=\"message service\" id=\"event{event_id}\" data-action=\"{action_slug}\"><header><b>{actor}</b><time>{date_iso}</time></header><div class=\"service details\">{}</div>{}</article>\n",
            html_escape(&entry.summary),
            render_field_changes(&entry.field_changes)
        ));
        return (html, 1);
    }

    let mut rendered_changes = false;
    for part in parts {
        count += 1;
        match part {
            AdminLogPart::Line {
                text, photo_file, ..
            } => {
                let mut extra = String::new();
                if let Some((rel, href)) = photo_file
                    .as_deref()
                    .and_then(clean_relative_path)
                    .and_then(|p| relative_href(&p).map(|href| (p, href)))
                {
                    extra.push_str(&format!(
                        "<div class=\"media_wrap\"><a class=\"media photo_wrap\" href=\"{}\">{}</a></div>",
                        html_escape(&href),
                        html_escape(&rel)
                    ));
                }
                if !rendered_changes {
                    rendered_changes = true;
                    extra.push_str(&render_field_changes(&entry.field_changes));
                }
                html.push_str(&format!(
                    "<article class=\"message service\" id=\"event{event_id}\" data-action=\"{action_slug}\"><header><b>{actor}</b><time>{date_iso}</time></header><div class=\"service details\">{}</div>{extra}</article>\n",
                    html_escape(text)
                ));
            }
            AdminLogPart::Note { parts: note_parts } => {
                let changes = if !rendered_changes {
                    rendered_changes = true;
                    render_field_changes(&entry.field_changes)
                } else {
                    String::new()
                };
                html.push_str(&format!(
                    "<article class=\"message default admin-note\" id=\"event{event_id}\" data-action=\"{action_slug}\"><header><b>{actor}</b><time>{date_iso}</time></header><div class=\"text\">{}</div>{changes}</article>\n",
                    render_admin_text_parts(note_parts)
                ));
            }
            AdminLogPart::Logged { snapshot } => {
                let sender = html_escape(&sender_display(snapshot, entry, peers));
                let ts = html_escape(&format_iso_utc(snapshot.date.unwrap_or(entry.timestamp)));
                let (clean_text, clean_entities) =
                    project_snapshot_text(&snapshot.text, &snapshot.entities);
                let body = render_entities(&clean_text, &clean_entities)
                    .unwrap_or_else(|_| html_escape(&clean_text));
                let media = render_snapshot_media(snapshot);
                html.push_str(&format!(
                    "<article class=\"message default bubble-after\" data-event-id=\"{event_id}\" data-snapshot=\"after\"><header><b>{sender}</b><time>{ts}</time></header><div class=\"text\">{body}</div>{media}</article>\n"
                ));
            }
            AdminLogPart::Original {
                title,
                parts: orig_parts,
                snapshot,
            } => {
                let sender_raw = snapshot
                    .as_ref()
                    .or(entry.before.as_ref())
                    .map(|s| sender_display(s, entry, peers))
                    .unwrap_or_else(|| {
                        display_name(Some(&entry.actor.name), DEFAULT_MAX_NAME_CHARS)
                    });
                let sender = html_escape(&sender_raw);
                let distinct_before = entry.before.as_ref().filter(|b| {
                    let after_file = entry.after.as_ref().and_then(|a| a.export_file.as_deref());
                    let after_thumb = entry
                        .after
                        .as_ref()
                        .and_then(|a| a.thumbnail_file.as_deref());
                    (b.export_file.is_some() && b.export_file.as_deref() != after_file)
                        || (b.thumbnail_file.is_some()
                            && b.thumbnail_file.as_deref() != after_thumb)
                });
                let media = snapshot
                    .as_ref()
                    .or(distinct_before)
                    .map(render_snapshot_media)
                    .unwrap_or_default();
                html.push_str(&format!(
                    "<article class=\"message default bubble-before\" data-event-id=\"{event_id}\" data-snapshot=\"before\"><header><b>{sender}</b><time>{date_iso}</time></header><div class=\"text\"><strong>{}</strong><br>{}</div>{media}</article>\n",
                    html_escape(title),
                    render_admin_text_parts(orig_parts)
                ));
            }
        }
    }
    (html, count)
}
