use super::admin_log::AdminLogEntry;
use super::admin_log_text::AdminLogPart;
use crate::text_fidelity::{DEFAULT_MAX_NAME_CHARS, display_name};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
#[path = "../../tests/unit/export_writer.rs"]
mod tests;

pub(super) fn title_for_peer(peer: &Value, peers: &BTreeMap<String, Value>) -> String {
    let id = peer
        .get("user_id")
        .or_else(|| peer.get("channel_id"))
        .or_else(|| peer.get("chat_id"));
    let kind = match peer["_"].as_str() {
        Some("inputPeerUser") => "user",
        Some("inputPeerChannel") => "channel",
        _ => "chat",
    };
    id.and_then(super::numeric_id)
        .and_then(|id| peers.get(&format!("{kind}:{id}")))
        .map(|value| {
            value["title"]
                .as_str()
                .map(|t| display_name(Some(t), DEFAULT_MAX_NAME_CHARS))
                .unwrap_or_else(|| super::render::person_name(value))
        })
        .unwrap_or_else(|| "Telegram chat export".into())
}

pub struct Writer {
    path: PathBuf,
    max_bytes: u64,
    json: Option<(tempfile::NamedTempFile, BufWriter<File>)>,
    html: Option<BufWriter<File>>,
    title: String,
    json_count: usize,
    html_articles: usize,
    html_page: usize,
    contacts_written: usize,
    admin_entries: Option<Vec<Value>>,
    next_admin_msg_id: i64,
}

impl Writer {
    pub fn new(
        path: &Path,
        settings: &super::settings::Settings,
        title: &str,
        peer: &Value,
    ) -> Result<Self> {
        Self::new_with_mode(path, settings, title, peer, false)
    }

    pub fn new_admin(
        path: &Path,
        settings: &super::settings::Settings,
        title: &str,
        peer: &Value,
    ) -> Result<Self> {
        Self::new_with_mode(path, settings, title, peer, true)
    }

    fn new_with_mode(
        path: &Path,
        settings: &super::settings::Settings,
        title: &str,
        peer: &Value,
        admin_mode: bool,
    ) -> Result<Self> {
        let json = if settings.format != "html" {
            let temporary = tempfile::NamedTempFile::new_in(path)?;
            let mut writer = BufWriter::new(temporary.as_file().try_clone()?);
            let peer_type = match peer["_"].as_str() {
                Some("inputPeerUser" | "inputPeerSelf") => "personal_chat",
                Some("inputPeerChannel") => "channel",
                _ => "private_group",
            };
            let header = json!({
                "name": title,
                "type": peer_type,
                "id": peer.get("user_id").or_else(|| peer.get("channel_id")).or_else(|| peer.get("chat_id")),
                "exporter": "telegram-mcp Rust",
            });
            let mut bytes = serde_json::to_vec_pretty(&header)?;
            bytes.pop();
            writer.write_all(&bytes)?;
            writer.write_all(b",\n\"messages\":[\n")?;
            Some((temporary, writer))
        } else {
            None
        };
        let html = if settings.format != "json" {
            Some(open_html(path, title, 1)?)
        } else {
            None
        };
        Ok(Self {
            path: path.to_owned(),
            max_bytes: settings.max_bytes,
            json,
            html,
            title: title.to_owned(),
            json_count: 0,
            html_articles: 0,
            html_page: 1,
            contacts_written: 0,
            admin_entries: admin_mode.then(Vec::new),
            next_admin_msg_id: 1,
        })
    }

    fn maybe_materialize_vcard(&mut self, message: &Value) -> Result<Option<(String, u64)>> {
        if message.get("contact_vcard").is_some() || message.get("export_file_skipped").is_some() {
            return Ok(None);
        }
        let media = &message["media"];
        if media["_"].as_str() != Some("messageMediaContact") {
            return Ok(None);
        }
        let Some(vcard) = media["vcard"].as_str().filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        self.contacts_written += 1;
        super::files::write_contact_vcard(&self.path, self.contacts_written, vcard, self.max_bytes)
    }

    fn rotate_html_if_needed(&mut self) -> Result<()> {
        if let Some(writer) = &mut self.html
            && self.html_articles > 0
            && self.html_articles.is_multiple_of(1000)
        {
            let next = self.html_page + 1;
            writer.write_all(
                format!(
                    "<nav><a href=\"{}\">Next messages</a></nav></html>",
                    html_name(next)
                )
                .as_bytes(),
            )?;
            writer.flush()?;
            self.html_page = next;
            *writer = open_html(&self.path, &self.title, next)?;
            writer.write_all(
                format!(
                    "<nav><a href=\"{}\">Previous messages</a></nav>",
                    html_name(next - 1)
                )
                .as_bytes(),
            )?;
        }
        Ok(())
    }

    pub fn message(&mut self, message: &Value, peers: &BTreeMap<String, Value>) -> Result<()> {
        let mut enriched = None;
        if let Some((rel, sz)) = self.maybe_materialize_vcard(message)? {
            let mut copy = message.clone();
            copy["contact_vcard"] = Value::String(rel);
            copy["contact_vcard_file_size"] = json!(sz);
            enriched = Some(copy);
        }
        let target = enriched.as_ref().unwrap_or(message);
        let html_chunk = if self.html.is_some() {
            Some(super::render::html_message(target, peers)?)
        } else {
            None
        };
        let json_record = if self.json.is_some() {
            Some(super::render::json_message(target, peers)?)
        } else {
            None
        };
        if let (Some((_, writer)), Some(rec)) = (&mut self.json, json_record) {
            if self.json_count > 0 {
                writer.write_all(b",\n")?;
            }
            serde_json::to_writer_pretty(writer, &rec)?;
            self.json_count += 1;
        }
        if let Some(chunk) = html_chunk {
            self.rotate_html_if_needed()?;
            if let Some(writer) = &mut self.html {
                writer.write_all(chunk.as_bytes())?;
            }
            self.html_articles += 1;
        }
        Ok(())
    }

    pub fn admin_event(
        &mut self,
        entry: &AdminLogEntry,
        parts: &[AdminLogPart],
        peers: &BTreeMap<String, Value>,
    ) -> Result<usize> {
        let mut emitted = 0;
        if let Some(ref mut entries) = self.admin_entries {
            entries.push(super::json_admin::serialize_admin_entry(entry));
        }
        let json_msgs = super::json_admin::render_event_messages(
            entry,
            parts,
            peers,
            &mut self.next_admin_msg_id,
        );
        emitted += json_msgs.len();
        if let Some((_, writer)) = &mut self.json {
            for msg in &json_msgs {
                if self.json_count > 0 {
                    writer.write_all(b",\n")?;
                }
                serde_json::to_writer_pretty(&mut *writer, msg)?;
                self.json_count += 1;
            }
        }
        if self.html.is_some() {
            let (chunk, articles) = super::html_admin::render_event_html(entry, parts, peers);
            self.rotate_html_if_needed()?;
            if let Some(writer) = &mut self.html {
                writer.write_all(chunk.as_bytes())?;
            }
            self.html_articles += articles.max(1);
        }
        Ok(emitted)
    }

    pub fn main_file_path(&self) -> PathBuf {
        if self.html.is_some() {
            self.path.join("messages.html")
        } else {
            self.path.join("result.json")
        }
    }

    pub fn write_dialog_slice(
        &mut self,
        messages: &[Value],
        peers: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let mut last_error: Option<anyhow::Error> = None;
        for msg in messages {
            if let Err(err) = self.message(msg, peers) {
                last_error = Some(err);
            }
        }
        if let Some(err) = last_error {
            return Err(err);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        let html_result: Result<()> = (|| {
            if let Some(mut writer) = self.html.take() {
                writer.write_all(b"</html>\n")?;
                writer.flush()?;
                writer.get_ref().sync_all()?;
            }
            Ok(())
        })();
        let json_result: Result<()> = (|| {
            if let Some((temporary, mut writer)) = self.json.take() {
                if let Some(entries) = self.admin_entries.take() {
                    writer.write_all(b"\n],\n\"events\": ")?;
                    serde_json::to_writer_pretty(&mut writer, &entries)?;
                    writer.write_all(b"\n}\n")?;
                } else {
                    writer.write_all(b"\n]}\n")?;
                }
                writer.flush()?;
                writer.get_ref().sync_all()?;
                drop(writer);
                temporary
                    .persist_noclobber(self.path.join("result.json"))
                    .context("Export JSON could not be finalized")?;
            }
            Ok(())
        })();
        match (html_result, json_result) {
            (_, Err(err)) => Err(err),
            (Err(err), Ok(())) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

fn html_name(page: usize) -> String {
    if page == 1 {
        "messages.html".into()
    } else {
        format!("messages{page}.html")
    }
}

fn open_html(path: &Path, title: &str, page: usize) -> Result<BufWriter<File>> {
    let file = File::options()
        .create_new(true)
        .write(true)
        .open(path.join(html_name(page)))?;
    let mut writer = BufWriter::new(file);
    writer.write_all(super::render::html_header(title).as_bytes())?;
    Ok(writer)
}
