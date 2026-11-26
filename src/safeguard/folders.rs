use crate::state::{State, canonical_existing, storage};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Component, Path, PathBuf};

const FILE: &str = "folder-grants.json";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderScope {
    Read,
    Write,
    ReadWrite,
}

impl FolderScope {
    fn covers(self, requested: Self) -> bool {
        self == Self::ReadWrite || self == requested
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FolderGrant {
    pub account: String,
    pub directory: PathBuf,
    pub scope: FolderScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    folders: Vec<FolderGrant>,
}

pub struct FolderGrants<'a> {
    state: &'a State,
    installation: PathBuf,
}

/// Own this guard inside one request. No account or task-wide permission is installed.
#[must_use]
pub struct TemporaryFolderGrant {
    files: Vec<(PathBuf, FolderGrant)>,
}

impl FolderGrants<'_> {
    pub(crate) fn resolve_for_call(&self, path: &Path) -> Result<PathBuf> {
        if path.is_absolute() {
            return self.resolve(path);
        }
        let root = self
            .state
            .file_roots()
            .first()
            .context("No allowed file roots are configured")?;
        self.resolve(&root.join(path))
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        ensure!(
            path.is_absolute() && !path.components().any(|p| matches!(p, Component::ParentDir)),
            "A folder decision requires an absolute path without traversal"
        );
        self.reject_protected(path)?;
        let path = canonical_existing(path)?;
        self.reject_protected(&path)?;
        Ok(path)
    }

    fn reject_protected(&self, path: &Path) -> Result<()> {
        ensure!(
            !path.components().any(protected_component),
            "The path names protected data"
        );
        ensure!(
            !within(path, self.state.root()),
            "The server state directory is protected"
        );
        ensure!(
            !within(path, &self.installation) || within(path, &self.installation.join("files")),
            "The installation is protected; use its files directory"
        );
        Ok(())
    }

    fn validate_directory(&self, directory: &Path) -> Result<PathBuf> {
        let directory = self.resolve(directory)?;
        ensure!(directory.is_dir(), "The approval folder must exist");
        ensure!(
            !within(self.state.root(), &directory) && !within(&self.installation, &directory),
            "The approval folder would cover protected roots"
        );
        Ok(directory)
    }

    fn load_unlocked(&self) -> Result<GrantFile> {
        let value = self.state.read_json_unlocked(FILE, json!({"folders":[]}))?;
        let grants: GrantFile = serde_json::from_value(value)?;
        if grants.folders.iter().any(|grant| {
            grant.request_id.is_some()
                || !grant.directory.is_absolute()
                || grant
                    .directory
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
                || crate::session::validate_label(&grant.account).is_err()
                || grant.account != grant.account.to_ascii_lowercase()
        }) {
            return Err(InvalidGrantFile.into());
        }
        Ok(grants)
    }

    fn save_unlocked(&self, grants: &GrantFile) -> Result<()> {
        let previous = self.state.read_json_unlocked(FILE, json!({"folders":[]}))?;
        if let Err(error) = self
            .state
            .write_json_unlocked(FILE, &serde_json::to_value(grants)?)
        {
            if error
                .downcast_ref::<storage::DurabilityUncertain>()
                .is_some()
            {
                self.state.write_json_unlocked(FILE, &previous)?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<FolderGrant>> {
        let _lock = self.state.state_lock()?;
        Ok(self.load_unlocked()?.folders)
    }

    /// Only an explicit owner `Always` decision uses this method.
    pub fn grant_always(&self, account: &str, directory: &Path, scope: FolderScope) -> Result<()> {
        let grant = FolderGrant {
            account: account_key(account)?,
            directory: self.validate_directory(directory)?,
            scope,
            request_id: None,
        };
        let _lock = self.state.state_lock()?;
        let mut grants = match self.load_unlocked() {
            Ok(grants) => grants,
            Err(error)
                if error.downcast_ref::<serde_json::Error>().is_some()
                    || error.downcast_ref::<std::str::Utf8Error>().is_some()
                    || error.downcast_ref::<InvalidGrantFile>().is_some() =>
            {
                let path = self.state.root().join(FILE);
                let meta = path.symlink_metadata()?;
                ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "Cannot set aside a nonregular grant record"
                );
                let backup = self.state.root().join(format!(
                    "folder-grants.corrupt-{:032x}.json",
                    rand::random::<u128>()
                ));
                ensure!(
                    !backup.try_exists()?,
                    "Corrupt grant backup name already exists"
                );
                std::fs::rename(&path, &backup)
                    .context("Cannot set aside corrupt folder grants")?;
                storage::sync_directory(self.state.root())?;
                GrantFile {
                    folders: Vec::new(),
                }
            }
            Err(error) => return Err(error),
        };
        if !grants.folders.iter().any(|old| {
            old.account == grant.account && old.scope == scope && old.directory == grant.directory
        }) {
            grants.folders.push(grant);
            self.save_unlocked(&grants)?;
        }
        Ok(())
    }

    /// Permit revoking an exact recorded directory even if it has disappeared.
    pub fn revoke(&self, account: &str, directory: &Path, scope: FolderScope) -> Result<bool> {
        let account = account_key(account)?;
        let resolved = self.resolve(directory)?;
        let _lock = self.state.state_lock()?;
        let mut grants = self.load_unlocked()?;
        let count = grants.folders.len();
        grants.folders.retain(|grant| {
            !(grant.account == account
                && grant.scope == scope
                && (grant.directory == directory || grant.directory == resolved))
        });
        if count == grants.folders.len() {
            return Ok(false);
        }
        self.save_unlocked(&grants)?;
        Ok(true)
    }

    /// An owner-approved or bypassed Once decision belongs to these exact files and this call.
    pub fn once(
        &self,
        request_id: &str,
        account: &str,
        paths: &[&Path],
        scope: FolderScope,
    ) -> Result<TemporaryFolderGrant> {
        ensure!(
            !request_id.is_empty() && request_id.len() <= 128 && !paths.is_empty(),
            "A temporary folder decision needs a specific request and paths"
        );
        let account = account_key(account)?;
        let files = paths
            .iter()
            .map(|path| {
                let resolved = self.resolve_for_call(path)?;
                let parent = resolved
                    .parent()
                    .context("An approved file needs a parent folder")?;
                let directory = self.validate_directory(parent)?;
                Ok((
                    resolved,
                    FolderGrant {
                        account: account.clone(),
                        directory,
                        scope,
                        request_id: Some(request_id.to_owned()),
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TemporaryFolderGrant { files })
    }

    /// Recheck identity before passing the resolved path to the anchored I/O layer.
    pub fn authorize(
        &self,
        request_id: &str,
        account: &str,
        path: &Path,
        scope: FolderScope,
        once: Option<&TemporaryFolderGrant>,
    ) -> Result<Option<PathBuf>> {
        let account = account_key(account)?;
        let resolved = self.resolve(path)?;
        if once.is_some_and(|guard| {
            guard.files.iter().any(|(file, grant)| {
                grant.account == account
                    && grant.request_id.as_deref() == Some(request_id)
                    && grant.scope.covers(scope)
                    && resolved == *file
                    && self.still_covers(grant, &resolved)
            })
        }) {
            return Ok(Some(resolved));
        }
        let granted = self.list()?.iter().any(|grant| {
            grant.account == account
                && grant.scope.covers(scope)
                && self.still_covers(grant, &resolved)
        });
        Ok(granted.then_some(resolved))
    }

    fn still_covers(&self, grant: &FolderGrant, resolved: &Path) -> bool {
        resolved.starts_with(&grant.directory)
            && self
                .validate_directory(&grant.directory)
                .is_ok_and(|directory| directory == grant.directory)
    }
}

impl<'a> FolderGrants<'a> {
    pub fn new(state: &'a State, installation: &Path) -> Result<Self> {
        Ok(Self {
            state,
            installation: installation.canonicalize()?,
        })
    }
}

#[derive(Debug)]
struct InvalidGrantFile;

impl std::fmt::Display for InvalidGrantFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Folder grant record is invalid")
    }
}

impl std::error::Error for InvalidGrantFile {}

fn account_key(account: &str) -> Result<String> {
    crate::session::validate_label(account)?;
    Ok(account.to_ascii_lowercase())
}

fn protected_component(component: Component<'_>) -> bool {
    let Component::Normal(name) = component else {
        return false;
    };
    let name = name.to_string_lossy();
    if cfg!(windows) && name.contains(':') {
        return true;
    }
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    matches!(
        name.as_str(),
        ".state"
            | ".ai"
            | ".git"
            | ".claude"
            | ".codex"
            | ".agents"
            | "safeguard"
            | "secrets.md"
            | "bypass.json"
            | "sealed.json"
            | "state.lock"
            | "folder-grants.json"
            | "always-approvals.json"
            | "account-rename-transaction.json"
    ) || name == ".env"
        || name.starts_with(".env.")
        || name.contains(".session")
        || name.starts_with("approval-")
        || name.starts_with("folder-grants.corrupt-")
}

pub(crate) fn tool_paths<'a>(
    tool: &str,
    args: &'a serde_json::Value,
) -> Vec<(&'a Path, FolderScope)> {
    use FolderScope::{Read, Write};
    let mut paths = Vec::new();
    for (key, scope) in [
        (
            "file_path",
            if matches!(
                tool,
                "download_media" | "download_rich_media" | "save_disappearing_media"
            ) {
                Write
            } else {
                Read
            },
        ),
        ("thumbnail_path", Read),
        ("output_path", Write),
        ("destination", Write),
        ("save_path", Write),
        ("description_file", Read),
        ("explanation_file", Read),
        ("folder", Write),
    ] {
        if (key != "folder" || tool == "start_secret_auto_save")
            && let Some(path) = args[key].as_str()
            // An empty export destination selects the server's default directory.
            && !(key == "destination" && path.is_empty() && matches!(tool, "export_chat_history" | "export_recent_actions" | "export_secret_chat"))
        {
            paths.push((Path::new(path), scope));
        }
    }
    for key in ["file_paths", "option_files"] {
        for path in args[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            paths.push((Path::new(path), Read));
        }
    }
    for path in args["rich_files"]
        .as_object()
        .into_iter()
        .flat_map(|files| files.values())
        .filter_map(serde_json::Value::as_str)
    {
        paths.push((Path::new(path), Read));
    }
    paths
}

pub(super) fn within(path: &Path, root: &Path) -> bool {
    let mut path = path.components();
    root.components().all(|part| {
        path.next().is_some_and(|component| {
            if cfg!(windows) {
                let left = component.as_os_str();
                let right = part.as_os_str();
                left == right
                    || left.to_string_lossy().to_lowercase()
                        == right.to_string_lossy().to_lowercase()
            } else {
                component == part
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<(tempfile::TempDir, State, PathBuf)> {
        let root = tempfile::tempdir()?;
        let installation = root.path().join("install");
        std::fs::create_dir_all(installation.join("files"))?;
        let state =
            State::at_with_installation(root.path().join("state"), vec![], installation.clone())?;
        Ok((root, state, installation))
    }

    #[test]
    fn once_is_exact_scoped_and_disappears_after_every_exit() -> Result<()> {
        let (root, state, installation) = fixture()?;
        let grants = FolderGrants::new(&state, &installation)?;
        let scope = FolderScope::Write;
        let approved = root.path().join("approved");
        std::fs::create_dir(&approved)?;
        let file = approved.join("file");
        let other = approved.join("second");
        let failed = (|| -> Result<()> {
            let guard = grants.once("call", "work", &[&file], scope)?;
            assert!(
                grants
                    .authorize("call", "WORK", &file, scope, Some(&guard))?
                    .is_some()
            );
            assert!(
                grants
                    .authorize("other", "work", &file, scope, Some(&guard))?
                    .is_none()
            );
            assert!(
                grants
                    .authorize("call", "home", &file, scope, Some(&guard))?
                    .is_none()
            );
            assert!(
                grants
                    .authorize("call", "work", &file, FolderScope::Read, Some(&guard))?
                    .is_none()
            );
            assert!(
                grants
                    .authorize("call", "work", &other, scope, Some(&guard))?
                    .is_none()
            );
            anyhow::bail!("Synthetic call failed")
        })();
        assert_eq!(failed.unwrap_err().to_string(), "Synthetic call failed");
        assert!(
            grants
                .authorize("call", "work", &file, scope, None)?
                .is_none()
        );
        assert!(grants.list()?.is_empty());
        Ok(())
    }

    #[test]
    fn durable_grants_are_canonical_and_revocation_applies_on_next_use() -> Result<()> {
        let (root, state, installation) = fixture()?;
        let directory = root.path().join("shared");
        std::fs::create_dir_all(directory.join("nested"))?;
        let file = directory.join("nested/example");
        FolderGrants::new(&state, &installation)?.grant_always(
            "WORK",
            &directory.join("."),
            FolderScope::ReadWrite,
        )?;
        let grants = FolderGrants::new(&state, &installation)?;
        assert_eq!(grants.list()?[0].directory, directory.canonicalize()?);
        assert!(
            grants
                .authorize("call", "work", &file, FolderScope::Read, None)?
                .is_some()
        );
        assert!(
            grants
                .authorize("call", "home", &file, FolderScope::Read, None)?
                .is_none()
        );
        assert!(grants.revoke("work", &directory, FolderScope::ReadWrite)?);
        assert!(!grants.revoke("work", &directory, FolderScope::ReadWrite)?);
        assert!(
            grants
                .authorize("call", "work", &file, FolderScope::Read, None)?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn protected_paths_cannot_be_approved_and_corrupt_grants_are_preserved() -> Result<()> {
        let (root, state, installation) = fixture()?;
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside)?;
        let grants = FolderGrants::new(&state, &installation)?;
        grants.grant_always("work", &outside, FolderScope::Read)?;
        for path in [
            outside.join(".ai/hidden"),
            outside.join(".STATE/item"),
            outside.join(".env.local"),
            outside.join("alias.session.json"),
            outside.join("safeguard/policy.rs"),
            installation.join("src/file"),
            state.root().join("custom"),
        ] {
            assert!(
                grants
                    .authorize("call", "work", &path, FolderScope::Read, None)
                    .is_err(),
                "{path:?}"
            );
            assert!(
                grants
                    .once("call", "work", &[&path], FolderScope::Read)
                    .is_err()
            );
        }
        assert!(
            grants
                .grant_always("work", state.root(), FolderScope::Read)
                .is_err()
        );
        for contents in [
            b"broken".as_slice(),
            br#"{"folders":[{"account":"work","directory":"relative","scope":"read"}]}"#,
        ] {
            std::fs::write(state.root().join(FILE), contents)?;
            assert!(grants.list().is_err());
            grants.grant_always("work", &outside, FolderScope::Write)?;
            assert_eq!(grants.list()?[0].scope, FolderScope::Write);
            let backups = std::fs::read_dir(state.root())?.collect::<std::io::Result<Vec<_>>>()?;
            assert!(
                backups
                    .iter()
                    .filter(|entry| entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("folder-grants.corrupt-"))
                    .any(|entry| std::fs::read(entry.path()).is_ok_and(|bytes| bytes == contents))
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_are_resolved_again_before_use() -> Result<()> {
        let (root, state, installation) = fixture()?;
        let directory = root.path().join("visible");
        std::fs::create_dir(&directory)?;
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&directory, &alias)?;
        let grants = FolderGrants::new(&state, &installation)?;
        grants.grant_always("work", &alias, FolderScope::Read)?;
        let file = alias.join("example");
        assert!(
            grants
                .authorize("call", "work", &file, FolderScope::Read, None)?
                .is_some()
        );
        std::fs::remove_file(&alias)?;
        std::os::unix::fs::symlink(state.root(), &alias)?;
        assert!(
            grants
                .authorize("call", "work", &file, FolderScope::Read, None)
                .is_err()
        );
        Ok(())
    }
}
