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
}

impl ExportFormat {
    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "html" => Ok(Self::Html),
            "txt" | "text" => Ok(Self::Text),
            _ => bail!("Secret-chat export format must be json, html, or text"),
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Text => "txt",
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
) -> Result<PathBuf> {
    let path = output(
        state,
        destination,
        &format!("secret-{label}-{chat}.{}", format.extension()),
    )?;
    let bytes = match format {
        ExportFormat::Json => serde_json::to_vec_pretty(&json!({"chat_id":chat,"messages":rows}))?,
        ExportFormat::Html => {
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
            .find_map(|key| row[key].as_str())
            .unwrap_or_default()
    )
}
