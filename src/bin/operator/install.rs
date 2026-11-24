use super::payload;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

fn directory(path: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    if path.symlink_metadata().is_ok() {
        ensure!(
            payload::plain(path)?.is_dir(),
            "Skill destination must be a directory"
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        directory(parent, created)?;
    }
    fs::create_dir(path)?;
    created.push(path.to_owned());
    Ok(())
}

fn is_legacy_config(path: &Path, expected: &Value) -> Result<bool> {
    match path.symlink_metadata() {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Cannot inspect a legacy MCP config"),
        Ok(_) => {}
    }
    let metadata = payload::plain(path)?;
    ensure!(
        metadata.is_file(),
        "Legacy MCP config is not a regular file"
    );
    if metadata.len() > 1_000_000 {
        return Ok(false);
    }
    let bytes = fs::read(path).context("Cannot read a legacy MCP config")?;
    Ok(serde_json::from_slice::<Value>(&bytes).is_ok_and(|config| &config == expected))
}

fn legacy_configs(destination: &Path) -> Result<Vec<(PathBuf, Value)>> {
    let mut obsolete = Vec::new();
    let standard = json!({"mcpServers": {"telegram-mcp": {"command": "telegram-mcp", "args": []}}});
    let mut portable = standard.clone();
    portable["$schema"] = json!("https://agent-plugins.org/schemas/1.0.0/mcp.schema.json");
    portable["mcpServers"]["telegram-mcp"]["type"] = json!("stdio");
    for (name, expected) in [
        (".mcp.json", &standard),
        ("mcp_config.json", &standard),
        ("mcp.json", &portable),
    ] {
        let path = destination.join(name);
        if is_legacy_config(&path, expected)? {
            obsolete.push((path, expected.clone()));
        }
    }
    Ok(obsolete)
}

fn remove_legacy_configs(paths: Vec<(PathBuf, Value)>) -> Result<()> {
    let mut staged: Vec<(PathBuf, Value, Vec<u8>)> = Vec::new();
    for (path, expected) in paths {
        if !is_legacy_config(&path, &expected)? {
            continue;
        }
        let bytes = fs::read(&path).context("Cannot back up a legacy MCP config before removal")?;
        staged.push((path, expected, bytes));
    }
    let mut removed: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for (path, expected, bytes) in staged {
        let step = (|| -> Result<()> {
            if is_legacy_config(&path, &expected)? {
                fs::remove_file(&path).context("Cannot remove a legacy MCP config")?;
            }
            Ok(())
        })();
        if let Err(error) = step {
            let mut rollback_failed = false;
            for (previous, payload) in removed.into_iter().rev() {
                if fs::write(previous, payload).is_err() {
                    rollback_failed = true;
                }
            }
            if rollback_failed {
                return Err(error.context(
                    "Legacy MCP config cleanup failed and restoring earlier files was incomplete",
                ));
            }
            return Err(error);
        }
        removed.push((path, bytes));
    }
    Ok(())
}

pub fn install(source: &Path, destination: &Path) -> Result<()> {
    install_with(source, destination, |file, data| {
        file.write_all(data)?;
        file.sync_all()?;
        Ok(())
    })
}

pub fn install_with<F>(source: &Path, destination: &Path, mut write: F) -> Result<()>
where
    F: FnMut(&mut fs::File, &[u8]) -> Result<()>,
{
    let source = payload::checked_source(source)?;
    let payload = payload::read_payload(&source)?;
    let destination = payload::absolute(destination)?;
    payload::outside_payload(&source, &destination)?;
    if destination.symlink_metadata().is_ok() {
        ensure!(
            payload::plain(&destination)?.is_dir(),
            "Skill destination must be a directory"
        );
    }
    let obsolete = if destination == source {
        Vec::new()
    } else {
        legacy_configs(&destination)?
    };
    if payload::SKILLS
        .iter()
        .any(|(name, _)| destination.join(name).symlink_metadata().is_ok())
    {
        let collision = "A target skill already exists; existing skills were not overwritten";
        ensure!(!obsolete.is_empty(), collision);
        for (name, data) in &payload {
            let Some(relative) = name.strip_prefix("skills/") else {
                continue;
            };
            let relative = Path::new(relative);
            payload::check_parents(&destination, relative).context(collision)?;
            let target = destination.join(relative);
            let metadata = payload::plain(&target).context(collision)?;
            ensure!(
                metadata.is_file()
                    && metadata.len() == data.len() as u64
                    && fs::read(target)? == *data,
                collision
            );
        }
        return remove_legacy_configs(obsolete);
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let result = (|| -> Result<()> {
        directory(&destination, &mut directories)?;
        for (name, _) in payload::SKILLS {
            let path = destination.join(name);
            fs::create_dir(&path)?;
            directories.push(path);
        }
        for (name, data) in &payload {
            let Some(relative) = name.strip_prefix("skills/") else {
                continue;
            };
            let target = destination.join(relative);
            directory(
                target.parent().expect("Skill payload has a parent"),
                &mut directories,
            )?;
            // Track a failed partially created file too, while never deleting an existing one.
            ensure!(
                target.symlink_metadata().is_err(),
                "A skill payload target already exists"
            );
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)?;
            files.push(target.clone());
            write(&mut file, data)?;
        }
        remove_legacy_configs(obsolete)
    })();
    if result.is_err() {
        let mut incomplete = false;
        for file in files.into_iter().rev() {
            if file.symlink_metadata().is_ok() && fs::remove_file(file).is_err() {
                incomplete = true;
            }
        }
        for path in directories.into_iter().rev() {
            if fs::remove_dir(path).is_err() {
                incomplete = true;
            }
        }
        ensure!(
            !incomplete,
            "Installation failed; owned rollback is incomplete; existing owner files were not changed"
        );
    }
    result
}
