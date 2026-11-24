use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

pub const SKILLS: [(&str, &str); 5] = [
    ("telegram-bot-reconstruction", "behavior-contract.md"),
    ("telegram-chat-migration", "migration-ledger.md"),
    ("telegram-backup-restore", "verification.md"),
    ("telegram-access-audit", "audit-matrix.md"),
    ("telegram-format-reconstruction", "fidelity.md"),
];
pub const TREES: [&str; 6] = [
    "skills",
    ".claude-plugin",
    ".codex-plugin",
    ".agents/plugins",
    ".cursor-plugin",
    ".antigravity-plugin",
];

pub fn client_directory(name: &str) -> Result<&'static str> {
    match name {
        "claude" => Ok(".claude/skills"),
        "codex" | "antigravity" => Ok(".agents/skills"),
        "cursor" => Ok(".cursor/skills"),
        "kiro" => Ok(".kiro/skills"),
        "cline" => Ok(".cline/skills"),
        "hermes" => Ok(".hermes/skills"),
        "opencode" => Ok(".opencode/skills"),
        _ => bail!(
            "Unknown skill client; choose claude, codex, cursor, kiro, cline, hermes, opencode or antigravity"
        ),
    }
}

pub fn files() -> BTreeSet<String> {
    let mut files = [
        "plugin.json",
        "docs/operator-plugin.md",
        ".claude-plugin/plugin.json",
        ".claude-plugin/marketplace.json",
        ".codex-plugin/plugin.json",
        ".agents/plugins/marketplace.json",
        ".cursor-plugin/plugin.json",
        ".cursor-plugin/marketplace.json",
        ".antigravity-plugin/plugin.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    for (name, reference) in SKILLS {
        files.insert(format!("skills/{name}/SKILL.md"));
        files.insert(format!("skills/{name}/references/{reference}"));
    }
    files
}

pub fn plain(path: &Path) -> Result<fs::Metadata> {
    let metadata =
        fs::symlink_metadata(path).context("A required payload path is missing or unreadable")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Linked or reparse payload paths are not allowed"
        );
    }
    ensure!(
        !metadata.file_type().is_symlink(),
        "Linked payload paths are not allowed"
    );
    ensure!(
        metadata.is_dir() || metadata.is_file(),
        "Only regular payload files and directories are allowed"
    );
    Ok(metadata)
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Resolve dot components before checking containment, including nonexistent output paths.
    let mut clean = PathBuf::new();
    for component in full.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(clean.pop(), "Path escapes its filesystem root");
            }
            other => clean.push(other.as_os_str()),
        }
    }
    for ancestor in clean.ancestors() {
        if ancestor.symlink_metadata().is_ok() {
            ensure!(
                plain(ancestor)?.is_dir() || ancestor == clean,
                "A destination ancestor is not a directory"
            );
        }
    }
    // Canonicalize the existing ancestor even when the destination does not
    // exist yet. On Windows this gives both paths the same filesystem identity
    // (including the verbatim prefix), rather than comparing C:\ with \\?\C:\.
    for ancestor in clean.ancestors() {
        if ancestor.symlink_metadata().is_ok() {
            let relative = clean.strip_prefix(ancestor)?;
            return Ok(ancestor
                .canonicalize()
                .context("Cannot resolve a destination ancestor")?
                .join(relative));
        }
    }
    bail!("Destination has no existing filesystem root")
}

pub fn checked_source(path: &Path) -> Result<PathBuf> {
    let path = absolute(path)?;
    ensure!(plain(&path)?.is_dir(), "Payload source is not a directory");
    path.canonicalize()
        .context("Cannot resolve the payload source")
}

pub fn outside_payload(source: &Path, destination: &Path) -> Result<()> {
    ensure!(
        !TREES
            .iter()
            .any(|tree| destination.starts_with(source.join(tree))),
        "Destination cannot change the canonical payload source"
    );
    Ok(())
}

pub(super) fn check_parents(source: &Path, path: &Path) -> Result<()> {
    let mut parent = source.to_owned();
    for component in path.parent().into_iter().flat_map(Path::components) {
        parent.push(component.as_os_str());
        ensure!(
            plain(&parent)?.is_dir(),
            "Every package document parent must be an unlinked directory"
        );
    }
    Ok(())
}

fn declares_mcp(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields.contains_key("mcpServers") || fields.values().any(declares_mcp)
        }
        Value::Array(items) => items.iter().any(declares_mcp),
        _ => false,
    }
}

pub fn read_payload(source: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let source = checked_source(source)?;
    let files = files();
    let mut directories = BTreeSet::new();
    for file in &files {
        for parent in Path::new(file).ancestors().skip(1) {
            directories.insert(parent.to_string_lossy().replace('\\', "/"));
        }
    }
    fn visit(
        path: &Path,
        root: &Path,
        files: &BTreeSet<String>,
        directories: &BTreeSet<String>,
    ) -> Result<()> {
        ensure!(
            plain(path)?.is_dir(),
            "Payload tree root is not a directory"
        );
        for item in fs::read_dir(path)? {
            let path = item?.path();
            let metadata = plain(&path)?;
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .context("Payload path is not UTF-8")?
                .replace('\\', "/");
            if metadata.is_dir() {
                ensure!(
                    directories.contains(&relative),
                    "Unexpected package directory"
                );
                visit(&path, root, files, directories)?;
            } else {
                ensure!(files.contains(&relative), "Unexpected package file");
            }
        }
        Ok(())
    }
    for tree in TREES {
        check_parents(&source, Path::new(tree))?;
        visit(&source.join(tree), &source, &files, &directories)?;
    }
    let mut result = BTreeMap::new();
    for file in files {
        check_parents(&source, Path::new(&file))?;
        let path = source.join(&file);
        let metadata = plain(&path)?;
        ensure!(
            metadata.is_file() && metadata.len() <= 1_000_000,
            "A package file is invalid or exceeds the byte limit"
        );
        let bytes = fs::read(path)?;
        std::str::from_utf8(&bytes).context("A package document is not UTF-8")?;
        if file.ends_with(".json") {
            let manifest: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid package JSON: {file}"))?;
            ensure!(
                !declares_mcp(&manifest),
                "Package manifest declares mcpServers: {file}"
            );
        }
        let key = if file == "docs/operator-plugin.md" {
            "README.md".to_owned()
        } else {
            file
        };
        result.insert(key, bytes);
    }
    Ok(result)
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_linked_parent_even_when_its_document_is_regular() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let source = root.path().join("source");
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in files()
            .into_iter()
            .filter(|name| name != "docs/operator-plugin.md")
        {
            let target = source.join(&name);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(repository.join(name), target).unwrap();
        }
        let docs = root.path().join("unrelated documents");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("operator-plugin.md"), b"Synthetic document").unwrap();
        let link = source.join("docs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&docs, &link).unwrap();
        #[cfg(windows)]
        assert!(
            std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&docs)
                .output()
                .unwrap()
                .status
                .success(),
            "Could not construct a directory junction fixture"
        );
        let error = read_payload(&source).unwrap_err();
        assert!(error.to_string().contains("Linked"), "{error:#}");
    }
}
