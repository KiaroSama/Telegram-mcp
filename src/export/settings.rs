use anyhow::{Context, Result, bail};
use chrono::TimeZone;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Settings {
    pub format: String,
    pub media: [bool; 7],
    pub max_bytes: u64,
    pub from: i64,
    pub until: i64,
    pub path: PathBuf,
}

impl Settings {
    pub fn parse(args: &Value) -> Result<Self> {
        let format = args["format"]
            .as_str()
            .context("Choose an export format: html, json or html_and_json")?;
        if !["html", "json", "html_and_json"].contains(&format) {
            bail!("Export format must be html, json or html_and_json");
        }
        let mut media = [false; 7];
        for (index, key) in [
            "photos",
            "videos",
            "voice_messages",
            "video_messages",
            "stickers",
            "gifs",
            "files",
        ]
        .iter()
        .enumerate()
        {
            media[index] = args[*key].as_bool().with_context(|| {
                format!("Choose whether to export {key}; a boolean is required")
            })?;
        }
        let megabytes = args["size_limit_mb"]
            .as_f64()
            .context("Choose size_limit_mb between 1 and 4000")?;
        if !megabytes.is_finite() || !(1.0..=4000.0).contains(&megabytes) {
            bail!("size_limit_mb must be between 1 and 4000");
        }
        let from = parse_date(args["date_from"].as_str().unwrap_or(""))?;
        let until = parse_date(args["date_to"].as_str().unwrap_or(""))?;
        if until > 0 && until <= from {
            bail!("date_to must be later than date_from");
        }
        let state = crate::state::State::from_environment()?;
        let destination = match args["destination"].as_str().filter(|s| !s.is_empty()) {
            Some(value) => {
                state.resolve_file(std::path::Path::new(value), crate::state::FileAccess::Write)?
            }
            None => state.default_downloads()?,
        };
        let base = destination.join(format!(
            "ChatExport_{}",
            chrono::Local::now().format("%Y-%m-%d")
        ));
        let mut path = base.clone();
        for suffix in 1..=10_000 {
            if !path.exists() {
                break;
            }
            path = base.with_file_name(format!(
                "{}_{}",
                base.file_name()
                    .context("Export folder has no name")?
                    .to_string_lossy(),
                suffix
            ));
        }
        if path.exists() {
            bail!("No unused export folder is available");
        }
        let path = state.resolve_file(&path, crate::state::FileAccess::Write)?;
        Ok(Self {
            format: format.to_owned(),
            media,
            max_bytes: (megabytes * 1024.0 * 1024.0) as u64,
            from,
            until,
            path,
        })
    }

    pub fn includes_date(&self, timestamp: i64) -> bool {
        timestamp >= self.from && (self.until == 0 || timestamp < self.until)
    }
}

pub fn parse_date(value: &str) -> Result<i64> {
    if value.is_empty() {
        return Ok(0);
    }
    let value = if value.len() == 10 {
        format!("{value} 00:00")
    } else {
        value.to_owned()
    };
    let date = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M")
        .context("Export dates must use YYYY-MM-DD or YYYY-MM-DD HH:MM")?;
    chrono::Local
        .from_local_datetime(&date)
        .single()
        .map(|date| date.timestamp())
        .context("Export date is ambiguous or nonexistent in the current local timezone")
}
