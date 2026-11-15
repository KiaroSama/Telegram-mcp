use super::payload;
use anyhow::{Result, ensure};
use std::{
    fs,
    io::Write,
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
    for (name, _) in payload::SKILLS {
        ensure!(
            destination.join(name).symlink_metadata().is_err(),
            "A target skill already exists; existing skills were not overwritten"
        );
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
        Ok(())
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
