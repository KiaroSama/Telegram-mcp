use super::payload;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{fs, io::Write, path::Path};
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

pub fn package(source: &Path, output: &Path, client: &str) -> Result<()> {
    ensure!(
        matches!(client, "portable" | "antigravity"),
        "Unknown plugin archive client"
    );
    let source = payload::checked_source(source)?;
    let mut payload = payload::read_payload(&source)?;
    let manifest: Value = serde_json::from_slice(
        payload
            .get("plugin.json")
            .context("Plugin manifest is missing")?,
    )?;
    ensure!(
        manifest["name"].as_str() == Some("telegram-operator"),
        "Unexpected plugin identity"
    );
    if client == "antigravity" {
        let manifest =
            serde_json::json!({"name":manifest["name"], "description":manifest["description"]});
        let mut text = serde_json::to_vec_pretty(&manifest)?;
        text.push(b'\n');
        payload.insert("plugin.json".into(), text);
        payload.retain(|name, _| {
            ![".claude-plugin/", ".codex-plugin/", ".cursor-plugin/"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        });
    }
    for name in ["LICENSE", "NOTICE"] {
        let path = source.join(name);
        ensure!(
            payload::plain(&path)?.is_file() && fs::metadata(&path)?.len() <= 1_000_000,
            "License file is invalid or exceeds byte limit"
        );
        let bytes = fs::read(path)?;
        std::str::from_utf8(&bytes).context("License file is not UTF-8")?;
        payload.insert(name.to_owned(), bytes);
    }
    let output = payload::absolute(output)?;
    payload::outside_payload(&source, &output)?;
    ensure!(
        output.symlink_metadata().is_err(),
        "Output already exists; choose a new archive path"
    );
    fs::create_dir_all(
        output
            .parent()
            .context("Archive destination has no parent")?,
    )?;
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)?;
    let result = (|| -> Result<()> {
        let mut archive = ZipWriter::new(file);
        let date = DateTime::from_date_and_time(2026, 11, 15, 0, 0, 0)?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(date)
            .unix_permissions(0o644);
        for (name, bytes) in payload {
            archive.start_file(format!("telegram-operator/{name}"), options)?;
            archive.write_all(&bytes)?;
        }
        archive.finish()?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        ensure!(
            fs::remove_file(&output).is_ok(),
            "Archive creation failed; incomplete owned output could not be removed"
        );
    }
    result
}
