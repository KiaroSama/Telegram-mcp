use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
#[path = "../../tests/unit/export_writer.rs"]
mod tests;

pub struct Writer {
    path: PathBuf,
    json: Option<(tempfile::NamedTempFile, BufWriter<File>)>,
    html: Option<BufWriter<File>>,
    title: String,
    count: usize,
    html_page: usize,
}

impl Writer {
    pub fn new(
        path: &Path,
        settings: &super::settings::Settings,
        title: &str,
        peer: &Value,
    ) -> Result<Self> {
        let json = if settings.format != "html" {
            let temporary = tempfile::NamedTempFile::new_in(path)?;
            let mut writer = BufWriter::new(temporary.as_file().try_clone()?);
            let header = json!({"name":title,"type":match peer["_"] .as_str(){Some("inputPeerUser"|"inputPeerSelf")=>"personal_chat",Some("inputPeerChannel")=>"channel",_=>"private_group"},
                "id":peer.get("user_id").or_else(||peer.get("channel_id")).or_else(||peer.get("chat_id")),
                "exporter":"telegram-mcp Rust"});
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
            json,
            html,
            title: title.to_owned(),
            count: 0,
            html_page: 1,
        })
    }

    pub fn message(&mut self, message: &Value, peers: &BTreeMap<String, Value>) -> Result<()> {
        if let Some((_, writer)) = &mut self.json {
            if self.count > 0 {
                writer.write_all(b",\n")?;
            }
            serde_json::to_writer_pretty(writer, &super::render::json_message(message, peers)?)?;
        }
        if let Some(writer) = &mut self.html {
            if self.count > 0 && self.count.is_multiple_of(1000) {
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
            writer.write_all(super::render::html_message(message, peers)?.as_bytes())?;
        }
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if let Some((temporary, mut writer)) = self.json.take() {
            writer.write_all(b"\n]}\n")?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            drop(writer);
            temporary
                .persist_noclobber(self.path.join("result.json"))
                .context("Export JSON could not be finalized")?;
        }
        if let Some(mut writer) = self.html.take() {
            writer.write_all(b"</html>\n")?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }
        Ok(())
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
