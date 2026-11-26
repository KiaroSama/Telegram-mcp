//! Document, contact vCard, relative-path map, and wire-credential sanitization helpers.

use super::{html_escape, relative_href};
use crate::export::files::clean_relative_path;
use crate::text_fidelity::{DEFAULT_MAX_NAME_CHARS, display_name};
use serde_json::{Map, Value, json};

pub(super) fn scrub_wire_credentials(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("access_hash");
            map.remove("file_reference");
            for val in map.values_mut() {
                scrub_wire_credentials(val);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_wire_credentials(item);
            }
        }
        _ => {}
    }
}

pub(super) fn push_document_fields(out: &mut Value, document: &Value) {
    let Some(attrs) = document["attributes"].as_array() else {
        return;
    };
    if let Some(size) = document["size"].as_u64() {
        out["file_size"] = json!(size);
    }
    if let Some(mime) = document["mime_type"].as_str().filter(|m| !m.is_empty()) {
        out["mime_type"] = Value::String(mime.to_owned());
    }
    let mut is_sticker = false;
    let mut is_anim = false;
    let mut is_voice = false;
    let mut is_round = false;
    let mut is_video = false;
    let mut is_audio = false;
    for attr in attrs {
        match attr["_"].as_str().unwrap_or_default() {
            "documentAttributeFilename" => {
                if let Some(name) = attr["file_name"].as_str().filter(|n| !n.is_empty()) {
                    out["file_name"] =
                        Value::String(display_name(Some(name), DEFAULT_MAX_NAME_CHARS));
                }
            }
            "documentAttributeSticker" | "documentAttributeCustomEmoji" => {
                is_sticker = true;
                if let Some(alt) = attr["alt"].as_str().filter(|a| !a.is_empty()) {
                    out["sticker_emoji"] = Value::String(alt.to_owned());
                }
            }
            "documentAttributeAnimated" => is_anim = true,
            "documentAttributeVideo" => {
                if attr["round_message"] == true {
                    is_round = true;
                } else {
                    is_video = true;
                }
                if let Some(d) = attr["duration"]
                    .as_f64()
                    .map(|d| d as i64)
                    .filter(|&d| d > 0)
                    .or_else(|| attr["duration"].as_i64().filter(|&d| d > 0))
                {
                    out["duration_seconds"] = json!(d);
                }
                if let (Some(w), Some(h)) = (attr["w"].as_u64(), attr["h"].as_u64())
                    && w > 0
                    && h > 0
                {
                    out["width"] = json!(w);
                    out["height"] = json!(h);
                }
            }
            "documentAttributeAudio" => {
                if attr["voice"] == true {
                    is_voice = true;
                } else {
                    is_audio = true;
                    if let Some(p) = attr["performer"].as_str().filter(|s| !s.is_empty()) {
                        out["performer"] =
                            Value::String(display_name(Some(p), DEFAULT_MAX_NAME_CHARS));
                    }
                    if let Some(t) = attr["title"].as_str().filter(|s| !s.is_empty()) {
                        out["title"] = Value::String(display_name(Some(t), DEFAULT_MAX_NAME_CHARS));
                    }
                }
                if let Some(d) = attr["duration"].as_i64().filter(|&d| d > 0) {
                    out["duration_seconds"] = json!(d);
                }
            }
            "documentAttributeImageSize" => {
                if let (Some(w), Some(h)) = (attr["w"].as_u64(), attr["h"].as_u64())
                    && w > 0
                    && h > 0
                {
                    out["width"] = json!(w);
                    out["height"] = json!(h);
                }
            }
            _ => {}
        }
    }
    let media_type = if is_sticker {
        Some("sticker")
    } else if is_round {
        Some("video_message")
    } else if is_voice {
        Some("voice_message")
    } else if is_anim {
        Some("animation")
    } else if is_video {
        Some("video_file")
    } else if is_audio {
        Some("audio_file")
    } else {
        None
    };
    if let Some(mt) = media_type {
        out["media_type"] = Value::String(mt.into());
    }
}

pub(super) fn push_contact_fields(out: &mut Value, message: &Value) {
    let media = &message["media"];
    if media["_"].as_str() != Some("messageMediaContact") {
        return;
    }
    let first = display_name(media["first_name"].as_str(), DEFAULT_MAX_NAME_CHARS);
    let last = display_name(media["last_name"].as_str(), DEFAULT_MAX_NAME_CHARS);
    let phone = display_name(media["phone_number"].as_str(), DEFAULT_MAX_NAME_CHARS);
    out["contact_information"] = json!({
        "first_name": first,
        "last_name": last,
        "phone_number": phone,
    });
    if let Some(vcard) = message
        .get("contact_vcard")
        .and_then(Value::as_str)
        .and_then(clean_relative_path)
    {
        out["contact_vcard"] = Value::String(vcard);
        if let Some(sz) = message["contact_vcard_file_size"].as_u64() {
            out["contact_vcard_file_size"] = json!(sz);
        }
    }
}

pub(super) fn sanitize_path_map(map_val: &Value) -> Option<Value> {
    let obj = map_val.as_object()?;
    let mut clean = Map::new();
    for (k, v) in obj {
        if let Some(rel) = v.as_str().and_then(clean_relative_path) {
            clean.insert(k.clone(), Value::String(rel));
        }
    }
    (!clean.is_empty()).then_some(Value::Object(clean))
}

pub(super) fn render_contact_html(record: &Value) -> String {
    let Some(info) = record.get("contact_information").and_then(Value::as_object) else {
        return String::new();
    };
    let first = info.get("first_name").and_then(Value::as_str).unwrap_or("");
    let last = info.get("last_name").and_then(Value::as_str).unwrap_or("");
    let full = [first, last]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let phone = info
        .get("phone_number")
        .and_then(Value::as_str)
        .unwrap_or("");
    let vcard_href = record
        .get("contact_vcard")
        .and_then(Value::as_str)
        .and_then(relative_href);
    match vcard_href {
        Some(href) => format!(
            "<div class=\"media_wrap\"><a class=\"media media_contact\" href=\"{}\"><strong class=\"contact-name\">{}</strong> <span class=\"contact-phone\">{} - vCard</span></a></div>",
            html_escape(&href),
            html_escape(&full),
            html_escape(phone)
        ),
        None => format!(
            "<div class=\"media_wrap\"><div class=\"media media_contact\"><strong class=\"contact-name\">{}</strong> <span class=\"contact-phone\">{}</span></div></div>",
            html_escape(&full),
            html_escape(phone)
        ),
    }
}
