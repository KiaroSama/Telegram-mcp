use anyhow::{Context, Result, bail};
use chrono::TimeZone;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static ACTIVE_FOLDERS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

fn active_folders() -> &'static Mutex<BTreeSet<PathBuf>> {
    ACTIVE_FOLDERS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn try_claim_active(path: &Path) -> bool {
    active_folders()
        .lock()
        .map(|mut set| set.insert(path.to_owned()))
        .unwrap_or(false)
}

fn release_active(path: &Path) {
    if let Ok(mut set) = active_folders().lock() {
        set.remove(path);
    }
}

fn create_dir_exclusive(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

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
        let planned = state.resolve_file(&self.path, crate::state::FileAccess::Write)?;
        let is_auto_child = planned
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ChatExport_"));

        if !is_auto_child {
            let parent = state.resolve_file(
                planned.parent().context("Export folder has no parent")?,
                crate::state::FileAccess::Write,
            )?;
            if !parent.exists() {
                crate::state::storage::private_directory(&parent)?;
            }
            let parent_guard = crate::transfer::files::Directory::open(&state, &parent)?;
            parent_guard.verify(&state)?;

            if !planned.exists() {
                match create_dir_exclusive(&planned) {
                    Ok(()) => {
                        if try_claim_active(&planned) {
                            let mut reservation = Reservation {
                                state,
                                parent: parent_guard,
                                directory: None,
                                path: planned.clone(),
                                created_dir: true,
                            };
                            crate::state::storage::restrict(&planned, true)?;
                            reservation.directory = Some(crate::transfer::files::Directory::open(
                                &reservation.state,
                                &planned,
                            )?);
                            self.path = planned;
                            return Ok(reservation);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(err) => return Err(err).context("Cannot reserve the export folder"),
                }
            }

            if planned.is_dir()
                && !super::files::directory_is_nonempty(&planned)?
                && try_claim_active(&planned)
            {
                match crate::transfer::files::Directory::open(&state, &planned) {
                    Ok(dir_guard) => {
                        let reservation = Reservation {
                            state,
                            parent: parent_guard,
                            directory: Some(dir_guard),
                            path: planned.clone(),
                            created_dir: false,
                        };
                        self.path = planned;
                        return Ok(reservation);
                    }
                    Err(err) => {
                        release_active(&planned);
                        return Err(err);
                    }
                }
            }
            return self.reserve_child_under(state, &planned, &super::files::chat_export_stamp());
        }

        let parent = state.resolve_file(
            planned.parent().context("Export folder has no parent")?,
            crate::state::FileAccess::Write,
        )?;
        let base = planned
            .file_name()
            .context("Export folder has no name")?
            .to_string_lossy()
            .into_owned();
        self.reserve_child_under(state, &parent, &base)
    }

    fn reserve_child_under(
        &mut self,
        state: crate::state::State,
        parent: &Path,
        base: &str,
    ) -> Result<Reservation> {
        if !parent.exists() {
            crate::state::storage::private_directory(parent)?;
        }
        let parent_guard = crate::transfer::files::Directory::open(&state, parent)?;
        for suffix in 0..=10_000 {
            parent_guard.verify(&state)?;
            let path = parent.join(if suffix == 0 {
                base.to_owned()
            } else {
                format!("{base}_{suffix}")
            });
            if state.resolve_file(&path, crate::state::FileAccess::Write)? != path {
                bail!("Export folder changed before reservation");
            }
            if !try_claim_active(&path) {
                continue;
            }
            match create_dir_exclusive(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    release_active(&path);
                    continue;
                }
                Err(error) => {
                    release_active(&path);
                    return Err(error).context("Cannot reserve the export folder");
                }
                Ok(()) => {}
            }
            let mut reservation = Reservation {
                state,
                parent: parent_guard,
                directory: None,
                path: path.clone(),
                created_dir: true,
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
        let path = super::files::plan_destination(&state, args["destination"].as_str())?;
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

/// Owns active reservation state; rollback removes only newly created empty folders.
pub(crate) struct Reservation {
    state: crate::state::State,
    parent: crate::transfer::files::Directory,
    directory: Option<crate::transfer::files::Directory>,
    path: PathBuf,
    created_dir: bool,
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
        drop(self.directory.take());
        if self.created_dir && intact {
            let _ = std::fs::remove_dir(&self.path);
        }
        release_active(&self.path);
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
