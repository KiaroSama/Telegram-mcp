//! Desktop-compatible `result.json` projection for typed Recent Actions (`AdminLogEntry`).

use super::admin_log::{AdminLogEntry, MessageSnapshot, MessageTextEntity};
use super::admin_log_text::{AdminLogPart, AdminTextKind, AdminTextPart};
use super::files::clean_relative_path;
use super::render::person_name;
use crate::text_fidelity::{
    DEFAULT_MAX_NAME_CHARS, ProjectionOptions, RawTextEntity, display_name, project_text,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

fn format_iso_utc(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn tl_kind_from_admin_entity(kind: &str) -> &'static str {
    match kind {
        "messageentitybold" | "bold" => "messageEntityBold",
        "messageentityitalic" | "italic" => "messageEntityItalic",
        "messageentityunderline" | "underline" => "messageEntityUnderline",
        "messageentitystrike" | "strike" => "messageEntityStrike",
        "messageentitycode" | "code" => "messageEntityCode",
        "messageentitypre" | "pre" => "messageEntityPre",
        "messageentityblockquote" | "blockquote" => "messageEntityBlockquote",
        "messageentityspoiler" | "spoiler" => "messageEntitySpoiler",
        "messageentitytexturl" | "text_url" => "messageEntityTextUrl",
        "messageentityurl" | "url" => "messageEntityUrl",
        "messageentityemail" | "email" => "messageEntityEmail",
        "messageentitymention" | "mention" => "messageEntityMention",
        "messageentitycustomemoji" | "custom_emoji" => "messageEntityCustomEmoji",
        _ => "messageEntityUnknown",
    }
}

pub(crate) fn project_snapshot_text(
    text: &str,
    entities: &[MessageTextEntity],
) -> (String, Vec<Value>) {
    let raw: Vec<RawTextEntity> = entities
        .iter()
        .map(|e| {
            let mut meta = Map::new();
            if let Some(ref url) = e.url {
                meta.insert("url".into(), Value::String(url.clone()));
            }
            if let Some(ref lang) = e.language {
                meta.insert("language".into(), Value::String(lang.clone()));
            }
            if let Some(ref doc) = e.document_id {
                meta.insert("document_id".into(), Value::String(doc.clone()));
            }
            RawTextEntity {
                kind: tl_kind_from_admin_entity(&e.kind).into(),
                offset: i64::try_from(e.offset).unwrap_or(0),
                length: i64::try_from(e.length).unwrap_or(0),
                metadata: meta,
            }
        })
        .collect();
    let projected = project_text(Some(text), &raw, ProjectionOptions::fidelity());
    let out_entities = projected
        .entities
        .into_iter()
        .map(|ent| {
            let mut obj = Map::new();
            obj.insert("_".into(), Value::String(ent.kind));
            obj.insert("offset".into(), json!(ent.offset));
            obj.insert("length".into(), json!(ent.length));
            for (k, v) in ent.metadata {
                obj.insert(k, v);
            }
            Value::Object(obj)
        })
        .collect();
    (projected.text, out_entities)
}

fn sanitize_snapshot_json(snapshot: &MessageSnapshot) -> Value {
    let (clean_text, clean_entities) = project_snapshot_text(&snapshot.text, &snapshot.entities);
    let mut obj = Map::new();
    if let Some(ref mid) = snapshot.message_id {
        obj.insert("message_id".into(), Value::String(mid.clone()));
    }
    if let Some(ref sid) = snapshot.sender_id {
        obj.insert("sender_id".into(), Value::String(sid.clone()));
    }
    if let Some(ts) = snapshot.date {
        obj.insert("date".into(), json!(ts));
    }
    obj.insert("text".into(), Value::String(clean_text));
    if !clean_entities.is_empty() {
        obj.insert("entities".into(), Value::Array(clean_entities));
    }
    obj.insert(
        "has_caption_media".into(),
        json!(snapshot.has_caption_media),
    );
    if let Some(ref mid) = snapshot.media_id {
        obj.insert("media_id".into(), Value::String(mid.clone()));
    }
    if let Some(ref mk) = snapshot.media_kind {
        obj.insert("media_kind".into(), Value::String(mk.clone()));
    }
    if let Some(file) = snapshot
        .export_file
        .as_deref()
        .and_then(clean_relative_path)
    {
        obj.insert("export_file".into(), Value::String(file));
    }
    if let Some(thumb) = snapshot
        .thumbnail_file
        .as_deref()
        .and_then(clean_relative_path)
    {
        obj.insert("thumbnail_file".into(), Value::String(thumb));
    }
    obj.insert("pinned".into(), json!(snapshot.pinned));
    Value::Object(obj)
}

/// Serializes a typed `AdminLogEntry` with relative-only media references and zero raw TL records.
pub fn serialize_admin_entry(entry: &AdminLogEntry) -> Value {
    let mut obj = Map::new();
    obj.insert("event_id".into(), Value::String(entry.event_id.clone()));
    obj.insert("timestamp".into(), json!(entry.timestamp));
    obj.insert(
        "date".into(),
        Value::String(format_iso_utc(entry.timestamp)),
    );
    let mut actor = Map::new();
    actor.insert("id".into(), Value::String(entry.actor.id.clone()));
    actor.insert(
        "name".into(),
        Value::String(display_name(
            Some(&entry.actor.name),
            DEFAULT_MAX_NAME_CHARS,
        )),
    );
    if let Some(ref u) = entry.actor.username {
        actor.insert("username".into(), Value::String(u.clone()));
    }
    obj.insert("actor".into(), Value::Object(actor));
    if let Some(ref target) = entry.target {
        obj.insert("target".into(), json!(target));
    }
    obj.insert(
        "action_kind".into(),
        Value::String(entry.action_kind.as_str().to_owned()),
    );
    obj.insert("summary".into(), Value::String(entry.summary.clone()));
    if !entry.field_changes.is_empty() {
        obj.insert("field_changes".into(), json!(entry.field_changes));
    }
    if let Some(ref before) = entry.before {
        obj.insert("before".into(), sanitize_snapshot_json(before));
    }
    if let Some(ref after) = entry.after {
        obj.insert("after".into(), sanitize_snapshot_json(after));
    }
    Value::Object(obj)
}

fn sender_pair(
    snapshot: &MessageSnapshot,
    fallback_actor: &AdminLogEntry,
    peers: &BTreeMap<String, Value>,
) -> (String, String) {
    if let Some(ref sid) = snapshot.sender_id {
        if let Some(user) = peers.get(&format!("user:{sid}")).or_else(|| peers.get(sid)) {
            return (person_name(user), format!("user{sid}"));
        }
        if let Some(chan) = peers.get(&format!("channel:{sid}")) {
            let title = chan["title"].as_str().unwrap_or("Channel");
            return (
                display_name(Some(title), DEFAULT_MAX_NAME_CHARS),
                format!("channel{sid}"),
            );
        }
        return (format!("user{sid}"), format!("user{sid}"));
    }
    (
        display_name(Some(&fallback_actor.actor.name), DEFAULT_MAX_NAME_CHARS),
        format!("user{}", fallback_actor.actor.id),
    )
}

fn parts_to_json_text(title: Option<&str>, parts: &[AdminTextPart]) -> (String, Value) {
    let mut flat = String::new();
    let mut structured = Vec::new();
    if let Some(t) = title {
        flat.push_str(t);
        flat.push('\n');
        structured.push(json!({"type": "bold", "text": t}));
        structured.push(Value::String("\n".into()));
    }
    for p in parts {
        flat.push_str(&p.text);
        match p.kind {
            AdminTextKind::Plain => structured.push(Value::String(p.text.clone())),
            AdminTextKind::Bold => structured.push(json!({"type": "bold", "text": p.text})),
            AdminTextKind::Italic => structured.push(json!({"type": "italic", "text": p.text})),
            AdminTextKind::Url => structured.push(json!({"type": "link", "text": p.text})),
        }
    }
    (flat, Value::Array(structured))
}

fn attach_snapshot_media(record: &mut Map<String, Value>, snapshot: &MessageSnapshot) {
    if let Some(rel) = snapshot
        .export_file
        .as_deref()
        .and_then(clean_relative_path)
    {
        let key = if snapshot.media_kind.as_deref() == Some("photo") {
            "photo"
        } else {
            "file"
        };
        record.insert(key.into(), Value::String(rel));
    }
    if let Some(thumb) = snapshot
        .thumbnail_file
        .as_deref()
        .and_then(clean_relative_path)
    {
        record.insert("thumbnail".into(), Value::String(thumb));
    }
}

/// Projects one `AdminLogEntry` and its `AdminLogPart` items into Desktop `result.json` messages.
pub fn render_event_messages(
    entry: &AdminLogEntry,
    parts: &[AdminLogPart],
    peers: &BTreeMap<String, Value>,
    next_id: &mut i64,
) -> Vec<Value> {
    let mut out = Vec::new();
    let date_iso = format_iso_utc(entry.timestamp);
    let date_unix = entry.timestamp.to_string();
    let actor_name = display_name(Some(&entry.actor.name), DEFAULT_MAX_NAME_CHARS);
    let actor_id = format!("user{}", entry.actor.id);

    if parts.is_empty() {
        let id = *next_id;
        *next_id += 1;
        out.push(json!({
            "id": id,
            "event_id": entry.event_id,
            "type": "service",
            "date": date_iso,
            "date_unixtime": date_unix,
            "actor": actor_name,
            "actor_id": actor_id,
            "action": entry.action_kind.as_str(),
            "information_text": entry.summary,
            "text": "",
            "text_entities": [],
        }));
        return out;
    }

    let mut attached_header_meta = false;
    for part in parts {
        let id = *next_id;
        *next_id += 1;
        match part {
            AdminLogPart::Line {
                text, photo_file, ..
            } => {
                let mut rec = Map::new();
                rec.insert("id".into(), json!(id));
                rec.insert("event_id".into(), Value::String(entry.event_id.clone()));
                rec.insert("type".into(), Value::String("service".into()));
                rec.insert("date".into(), Value::String(date_iso.clone()));
                rec.insert("date_unixtime".into(), Value::String(date_unix.clone()));
                rec.insert("actor".into(), Value::String(actor_name.clone()));
                rec.insert("actor_id".into(), Value::String(actor_id.clone()));
                rec.insert(
                    "action".into(),
                    Value::String(entry.action_kind.as_str().into()),
                );
                rec.insert("information_text".into(), Value::String(text.clone()));
                rec.insert("text".into(), Value::String(String::new()));
                rec.insert("text_entities".into(), json!([]));
                if let Some(photo) = photo_file.as_deref().and_then(clean_relative_path) {
                    rec.insert("photo".into(), Value::String(photo));
                }
                if !attached_header_meta {
                    attached_header_meta = true;
                    if let Some(ref tgt) = entry.target {
                        rec.insert("target".into(), json!(tgt));
                    }
                    if !entry.field_changes.is_empty() {
                        rec.insert("field_changes".into(), json!(entry.field_changes));
                    }
                }
                out.push(Value::Object(rec));
            }
            AdminLogPart::Note { parts: note_parts } => {
                let (flat, structured) = parts_to_json_text(None, note_parts);
                let mut rec = Map::new();
                rec.insert("id".into(), json!(id));
                rec.insert("event_id".into(), Value::String(entry.event_id.clone()));
                rec.insert("type".into(), Value::String("message".into()));
                rec.insert("date".into(), Value::String(date_iso.clone()));
                rec.insert("date_unixtime".into(), Value::String(date_unix.clone()));
                rec.insert("from".into(), Value::String(actor_name.clone()));
                rec.insert("from_id".into(), Value::String(actor_id.clone()));
                rec.insert(
                    "action_kind".into(),
                    Value::String(entry.action_kind.as_str().into()),
                );
                rec.insert("text".into(), structured);
                rec.insert("plain_text".into(), Value::String(flat));
                rec.insert("text_entities".into(), json!([]));
                if !attached_header_meta {
                    attached_header_meta = true;
                    if let Some(ref tgt) = entry.target {
                        rec.insert("target".into(), json!(tgt));
                    }
                    if !entry.field_changes.is_empty() {
                        rec.insert("field_changes".into(), json!(entry.field_changes));
                    }
                }
                out.push(Value::Object(rec));
            }
            AdminLogPart::Logged { snapshot } => {
                let (from, from_id) = sender_pair(snapshot, entry, peers);
                let (clean_text, clean_entities) =
                    project_snapshot_text(&snapshot.text, &snapshot.entities);
                let ts = snapshot.date.unwrap_or(entry.timestamp);
                let mut rec = Map::new();
                rec.insert("id".into(), json!(id));
                rec.insert("event_id".into(), Value::String(entry.event_id.clone()));
                rec.insert("snapshot_role".into(), Value::String("after".into()));
                if let Some(ref mid) = snapshot.message_id {
                    rec.insert("source_message_id".into(), Value::String(mid.clone()));
                }
                rec.insert("type".into(), Value::String("message".into()));
                rec.insert("date".into(), Value::String(format_iso_utc(ts)));
                rec.insert("date_unixtime".into(), Value::String(ts.to_string()));
                rec.insert("from".into(), Value::String(from));
                rec.insert("from_id".into(), Value::String(from_id));
                rec.insert("text".into(), Value::String(clean_text));
                rec.insert("text_entities".into(), Value::Array(clean_entities));
                attach_snapshot_media(&mut rec, snapshot);
                out.push(Value::Object(rec));
            }
            AdminLogPart::Original {
                title,
                parts: orig_parts,
                snapshot,
            } => {
                let (from, from_id) = snapshot
                    .as_ref()
                    .or(entry.before.as_ref())
                    .map(|s| sender_pair(s, entry, peers))
                    .unwrap_or_else(|| (actor_name.clone(), actor_id.clone()));
                let (flat, structured) = parts_to_json_text(Some(title), orig_parts);
                let mut rec = Map::new();
                rec.insert("id".into(), json!(id));
                rec.insert("event_id".into(), Value::String(entry.event_id.clone()));
                rec.insert("snapshot_role".into(), Value::String("before".into()));
                rec.insert("original_title".into(), Value::String(title.clone()));
                if let Some(mid) = snapshot
                    .as_ref()
                    .or(entry.before.as_ref())
                    .and_then(|s| s.message_id.clone())
                {
                    rec.insert("source_message_id".into(), Value::String(mid));
                }
                rec.insert("type".into(), Value::String("message".into()));
                rec.insert("date".into(), Value::String(date_iso.clone()));
                rec.insert("date_unixtime".into(), Value::String(date_unix.clone()));
                rec.insert("from".into(), Value::String(from));
                rec.insert("from_id".into(), Value::String(from_id));
                rec.insert("text".into(), structured);
                rec.insert("plain_text".into(), Value::String(flat));
                rec.insert("text_entities".into(), json!([]));
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
                if let Some(snap) = snapshot.as_ref().or(distinct_before) {
                    attach_snapshot_media(&mut rec, snap);
                }
                out.push(Value::Object(rec));
            }
        }
    }
    out
}
