use super::{history::mkdir, write_atomic};
use crate::state::{FileAccess, State};
use anyhow::{Context, Result, bail};
use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Html,
    Text,
    HtmlAndJson,
}

impl ExportFormat {
    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "html" => Ok(Self::Html),
            "txt" | "text" => Ok(Self::Text),
            "html_and_json" => Ok(Self::HtmlAndJson),
            _ => bail!("Secret-chat export format must be json, html, html_and_json, or text"),
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html | Self::HtmlAndJson => "html",
            Self::Text => "txt",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExportFilter {
    pub date_from: Option<chrono::DateTime<chrono::Utc>>,
    pub date_to: Option<chrono::DateTime<chrono::Utc>>,
    pub photos: bool,
    pub videos: bool,
    pub voice_messages: bool,
    pub video_messages: bool,
    pub stickers: bool,
    pub gifs: bool,
    pub files: bool,
    pub size_limit_bytes: Option<usize>,
}

impl ExportFilter {
    pub(super) fn from_args(args: &Value) -> Result<Self> {
        let parse_opt = |field: &str| -> Result<Option<chrono::DateTime<chrono::Utc>>> {
            let Some(raw) = args[field].as_str().filter(|s| !s.is_empty()) else {
                return Ok(None);
            };
            let ts = crate::export::settings::parse_date(raw)?;
            Ok(Some(
                chrono::DateTime::from_timestamp(ts, 0)
                    .context("Secret-chat export date is out of range")?,
            ))
        };
        let size_limit_bytes = args["size_limit_mb"]
            .as_f64()
            .filter(|v| *v > 0.0)
            .map(|mb| (mb * 1024.0 * 1024.0) as usize);
        let flag = |field: &str, default: bool| args[field].as_bool().unwrap_or(default);
        let default_media = args["include_media"].as_bool().unwrap_or(true);
        Ok(Self {
            date_from: parse_opt("date_from")?,
            date_to: parse_opt("date_to")?,
            photos: flag("photos", default_media),
            videos: flag("videos", default_media),
            voice_messages: flag("voice_messages", default_media),
            video_messages: flag("video_messages", default_media),
            stickers: flag("stickers", default_media),
            gifs: flag("gifs", default_media),
            files: flag("files", default_media),
            size_limit_bytes,
        })
    }

    pub(super) fn matches_row(&self, row: &Value) -> bool {
        if self.date_from.is_none() && self.date_to.is_none() {
            return true;
        }
        let Some(date) = row["date"]
            .as_str()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        else {
            return false;
        };
        if self.date_from.is_some_and(|from| date < from) {
            return false;
        }
        if self.date_to.is_some_and(|to| date > to) {
            return false;
        }
        true
    }

    pub(super) fn allows_media(&self, row: &Value, bytes_len: usize) -> bool {
        if self.size_limit_bytes.is_some_and(|limit| bytes_len > limit) {
            return false;
        }
        match row["type"].as_str().unwrap_or("document") {
            "photo" => self.photos,
            "video" => self.videos,
            "voice" | "voice_note" => self.voice_messages,
            "video_note" => self.video_messages,
            "sticker" => self.stickers,
            "gif" | "animation" => self.gifs,
            "text" | "service" => false,
            _ => self.files,
        }
    }
}

pub(super) fn output(state: &State, requested: Option<&Path>, filename: &str) -> Result<PathBuf> {
    let path = match requested {
        Some(path) => {
            let path = state.resolve_file(path, FileAccess::Write)?;
            if path.is_dir() {
                path.join(filename)
            } else {
                path
            }
        }
        None => mkdir(state, &state.default_downloads()?)?.join(filename),
    };
    let path = state.resolve_file(&path, FileAccess::Write)?;
    let parent = path.parent().context("Secret-chat output has no parent")?;
    mkdir(state, parent)?;
    state.resolve_file(&path, FileAccess::Write)
}

pub(super) fn key_picture(state: &State, name: &str, hash: &[u8]) -> Result<PathBuf> {
    let visualization = kiaro_secret_chat::visualization::key_visualization(hash)?;
    use sha2::{Digest, Sha256};
    let fingerprint = crate::lowercase_hex(&Sha256::digest(hash)[..12]);
    let path = output(state, None, &format!("{name}-{fingerprint}.png"))?;
    let palette = [
        [255, 255, 255],
        [213, 230, 243],
        [45, 87, 117],
        [47, 153, 201],
    ];
    let mut pixels = Vec::with_capacity(120 * 120 * 3);
    for y in 0..120 {
        for x in 0..120 {
            pixels.extend(palette[usize::from(visualization.rows[y / 10][x / 10])]);
        }
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(&pixels, 120, 120, ColorType::Rgb8.into())?;
    if super::read_private(&path, super::MAX_BYTES)?.as_deref() != Some(png.as_slice()) {
        write_atomic(&path, &png, false)?;
    }
    Ok(path)
}

pub(super) fn write_history(
    state: &State,
    label: &str,
    chat: i32,
    rows: &[Value],
    format: ExportFormat,
    destination: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    if format == ExportFormat::HtmlAndJson {
        let html_path = write_single(state, label, chat, rows, ExportFormat::Html, destination)?;
        let json_dest = destination.map(|p| {
            if p.is_dir() {
                p.to_path_buf()
            } else {
                p.with_extension("json")
            }
        });
        let json_path = write_single(
            state,
            label,
            chat,
            rows,
            ExportFormat::Json,
            json_dest.as_deref(),
        )?;
        return Ok(vec![html_path, json_path]);
    }
    Ok(vec![write_single(
        state,
        label,
        chat,
        rows,
        format,
        destination,
    )?])
}

fn write_single(
    state: &State,
    label: &str,
    chat: i32,
    rows: &[Value],
    format: ExportFormat,
    destination: Option<&Path>,
) -> Result<PathBuf> {
    let path = output(
        state,
        destination,
        &format!("secret-{label}-{chat}.{}", format.extension()),
    )?;
    let bytes = match format {
        ExportFormat::Json => serde_json::to_vec_pretty(&json!({"chat_id":chat,"messages":rows}))?,
        ExportFormat::Html | ExportFormat::HtmlAndJson => {
            let mut html = String::from(
                "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Secret chat export</title><main><h1>Secret chat export</h1><ol>",
            );
            for row in rows {
                html.push_str("<li><pre>");
                for ch in display(row).chars() {
                    match ch {
                        '<' => html.push_str("&lt;"),
                        '>' => html.push_str("&gt;"),
                        '&' => html.push_str("&amp;"),
                        _ => html.push(ch),
                    }
                }
                html.push_str("</pre></li>");
            }
            html.push_str("</ol></main></html>\n");
            html.into_bytes()
        }
        ExportFormat::Text => rows
            .iter()
            .map(display)
            .collect::<Vec<_>>()
            .join("\n\n")
            .into_bytes(),
    };
    write_atomic(&path, &bytes, false)?;
    Ok(path)
}

fn display(row: &Value) -> String {
    format!(
        "{} {}\n{}",
        row["date"].as_str().unwrap_or_default(),
        if row["is_outgoing"] == true {
            "Outgoing"
        } else {
            "Incoming"
        },
        ["text", "caption", "action"]
            .iter()
            .filter_map(|key| row[key].as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    )
}
