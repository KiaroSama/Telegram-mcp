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
    pub(crate) fn reserve(&mut self) -> Result<Reservation> {
        let state = crate::state::State::from_environment()?;
        let parent = state.resolve_file(
            self.path.parent().context("Export folder has no parent")?,
            crate::state::FileAccess::Write,
        )?;
        if !parent.exists() {
            crate::state::storage::private_directory(&parent)?;
        }
        let parent_guard = crate::transfer::files::Directory::open(&state, &parent)?;
        let base = self
            .path
            .file_name()
            .context("Export folder has no name")?
            .to_string_lossy();
        for suffix in 0..=10_000 {
            parent_guard.verify(&state)?;
            let path = parent.join(if suffix == 0 {
                base.to_string()
            } else {
                format!("{base}_{suffix}")
            });
            if state.resolve_file(&path, crate::state::FileAccess::Write)? != path {
                bail!("Export folder changed before reservation");
            }
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("Cannot reserve the export folder"),
                Ok(()) => {}
            }
            let mut reservation = Reservation {
                state,
                parent: parent_guard,
                directory: None,
                path: path.clone(),
            };
            crate::state::storage::restrict(&path, true)?;
            reservation.directory = Some(crate::transfer::files::Directory::open(
                &reservation.state,
                &path,
            )?);
            self.path = path;
            return Ok(reservation);
        }
        bail!("No unused export folder is available")
    }

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

/// Owns only the newly created folder; rollback never removes partial output.
pub(crate) struct Reservation {
    state: crate::state::State,
    parent: crate::transfer::files::Directory,
    directory: Option<crate::transfer::files::Directory>,
    path: PathBuf,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let intact = self.parent.verify(&self.state).is_ok()
            && self
                .directory
                .as_ref()
                .is_none_or(|directory| directory.verify(&self.state).is_ok())
            && self
                .state
                .resolve_file(&self.path, crate::state::FileAccess::Write)
                .is_ok_and(|path| path == self.path);
        // Windows denies directory replacement while the guard is held, so
        // release it only after verification and immediately before removal.
        drop(self.directory.take());
        if intact {
            let _ = std::fs::remove_dir(&self.path);
        }
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
