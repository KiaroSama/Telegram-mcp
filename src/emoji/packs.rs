use super::vision;
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

pub const ENV_VAR: &str = "TELEGRAM_MCP_EMOJI_PACKS";
const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    pub id: String,
    pub pack: String,
    pub title: Value,
    pub family: Value,
    pub link: Value,
    pub glyph: Value,
    pub role: Value,
    pub name: Value,
    pub format: Value,
    pub thumb: Option<PathBuf>,
    pub stamp: Option<String>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Refresh {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub skipped: usize,
    pub problems: Vec<String>,
}

pub fn read_json(path: &Path) -> Result<Value> {
    let file = std::fs::File::open(path).context("Cannot open emoji JSON file")?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= MAX_JSON_BYTES,
        "Emoji JSON file exceeds the 64 MiB limit or is not a regular file"
    );
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_JSON_BYTES,
        "Emoji JSON file exceeds the byte limit"
    );
    serde_json::from_str(std::str::from_utf8(&bytes).context("Emoji JSON file is not UTF-8")?)
        .context("Emoji JSON file is invalid")
}

pub fn find_root(explicit: Option<&Path>) -> Result<PathBuf> {
    let root=explicit.map(Path::to_owned).or_else(||std::env::var_os(ENV_VAR).map(PathBuf::from))
        .with_context(||format!("No packs directory. Pass --packs-dir or set {ENV_VAR} to the Emoji Mapper export folder"))?;
    ensure!(
        root.join("index.json").is_file(),
        "{} has no index.json; it is not an Emoji Mapper export",
        root.display()
    );
    root.canonicalize()
        .context("Cannot resolve the emoji packs directory")
}

pub fn stamp_of(path: &Path) -> Result<String> {
    let metadata = path.metadata()?;
    let modified = metadata.modified()?;
    let nanos = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(value) => value.as_nanos() as i128,
        Err(value) => -(value.duration().as_nanos() as i128),
    };
    Ok(format!("{}:{nanos}", metadata.len()))
}

pub fn decimal_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(|id| id.parse::<i64>().ok())
        .or_else(|| value.as_i64())
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
}

pub fn thumb_for(root: &Path, id: &str) -> Result<Option<PathBuf>> {
    ensure!(
        id.parse::<i64>().is_ok_and(|id| id > 0),
        "An emoji id must be a positive 64-bit decimal string"
    );
    for name in [
        format!("{id}.webp"),
        format!("{id}_still.webp"),
        format!("{id}.webm"),
    ] {
        let path = root.join(".thumbs").join(name);
        if path.is_file() {
            let path = path.canonicalize()?;
            ensure!(
                path.starts_with(root.canonicalize()?),
                "An emoji thumbnail escapes its export directory"
            );
            return Ok(Some(path));
        }
    }
    Ok(None)
}

pub fn read_catalogue(root: &Path) -> Result<(BTreeMap<String, Entry>, Vec<String>)> {
    let index = read_json(&root.join("index.json"))?;
    let mut entries = BTreeMap::new();
    let mut problems = Vec::new();
    let catalogue = index["packs"]
        .as_array()
        .context("Emoji catalogue index must contain a packs array")?;
    for pack in catalogue {
        let Some(name) = pack["set_name"].as_str().filter(|name| !name.is_empty()) else {
            continue;
        };
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            problems.push(format!("{name}: unsafe sticker-set name"));
            continue;
        }
        let detail = match read_json(&root.join(format!("{name}.json"))) {
            Ok(value) => value,
            Err(error) => {
                problems.push(format!("{name}: {error}"));
                continue;
            }
        };
        let Some(items) = detail["emoji"].as_array() else {
            problems.push(format!("{name}: catalogue has no emoji array"));
            continue;
        };
        for emoji in items {
            let Some(id) = decimal_id(&emoji["custom_emoji_id"]) else {
                problems.push(format!("{name}: invalid custom emoji id"));
                continue;
            };
            let thumb = match thumb_for(root, &id) {
                Ok(path) => path,
                Err(error) => {
                    problems.push(format!("{id}: {error}"));
                    None
                }
            };
            let stamp = thumb.as_ref().map(|path| stamp_of(path)).transpose()?;
            entries.insert(
                id.clone(),
                Entry {
                    id,
                    pack: name.into(),
                    title: pack["title"].clone(),
                    family: pack["family"].clone(),
                    link: pack["link"].clone(),
                    glyph: emoji["glyph"].clone(),
                    role: emoji["role"].clone(),
                    name: emoji["name"].clone(),
                    format: emoji["format"].clone(),
                    thumb,
                    stamp,
                },
            );
        }
    }
    Ok((entries, problems))
}

pub fn source_map(root: &Path) -> Result<BTreeMap<String, String>> {
    let value = read_json(&root.join("index.json"))?;
    Ok(value["by_source_id"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(source, target)| {
            Some((
                decimal_id(&Value::String(source.clone()))?,
                decimal_id(target)?,
            ))
        })
        .collect())
}

pub fn ffmpeg_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FFMPEG")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

pub fn alpha_weight(image: &DynamicImage) -> u64 {
    image
        .to_rgba8()
        .pixels()
        .map(|pixel| u64::from(pixel[3]))
        .sum()
}

pub async fn open_picture(path: &Path) -> Result<DynamicImage> {
    let metadata = std::fs::metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
        "Emoji picture exceeds the file byte limit"
    );
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("webm"))
    {
        return best_video_frame(path).await;
    }
    let bytes = std::fs::read(path)?;
    let suffix = if bytes.starts_with(b"RIFF") && bytes.windows(4).any(|part| part == b"ANIM") {
        Some("webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"\x89PNG") && bytes.windows(4).any(|part| part == b"acTL") {
        Some("png")
    } else {
        None
    };
    if suffix.is_some() {
        let result=crate::tools::visual::capture::run_child(json!({"tool":"__decode_media","path":path.canonicalize()?,"count":6,"max_dimension":256})).await?;
        let mut best = None;
        let mut weight = 0;
        for frame in result
            .as_array()
            .context("Emoji animation worker returned no frames")?
        {
            let bytes = base64::engine::general_purpose::STANDARD.decode(
                frame["data"]
                    .as_str()
                    .context("Emoji animation worker omitted frame pixels")?,
            )?;
            let image = crate::tools::visual::images::decode_image(&bytes)?;
            let visible = alpha_weight(&image);
            if best.is_none() || visible > weight {
                best = Some(image);
                weight = visible;
            }
        }
        return best.context("Emoji animation produced no frames");
    }
    crate::tools::visual::images::decode_image(&bytes)
}

pub fn video_arguments(input: &Path, output: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-max_alloc",
        "67108864",
        "-max_pixels",
        "50000000",
        "-protocol_whitelist",
        "file,pipe",
        "-format_whitelist",
        "matroska,webm",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-c:v",
        "libvpx-vp9",
        "-i",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(input.into());
    args.extend(
        [
            "-frames:v",
            "30",
            "-fps_mode",
            "passthrough",
            "-threads",
            "1",
            "-vf",
            "scale=256:256:force_original_aspect_ratio=decrease",
        ]
        .into_iter()
        .map(Into::into),
    );
    args.push(output.into());
    args
}

async fn best_video_frame(path: &Path) -> Result<DynamicImage> {
    let executable = ffmpeg_path().context(
        "FFmpeg is required for WebM emoji thumbnails; install it and run refresh again",
    )?;
    let scratch = tempfile::tempdir()?;
    let mut child = crate::tools::messaging::media::child(executable)
        .args(video_arguments(path, &scratch.path().join("%03d.png")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Could not start the WebM emoji decoder")?;
    match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
        Ok(status) => ensure!(
            status?.success(),
            "The WebM emoji decoder failed; no thumbnail was indexed"
        ),
        Err(_) => {
            let _ = child.kill().await;
            bail!("The WebM emoji decoder exceeded 30 seconds");
        }
    }
    let mut best = None;
    let mut weight = 0;
    for index in 1..=30 {
        let frame = scratch.path().join(format!("{index:03}.png"));
        if !frame.is_file() {
            continue;
        }
        let bytes = std::fs::read(frame)?;
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "Decoded emoji frame exceeds the output byte limit"
        );
        let image = crate::tools::visual::images::decode_image(&bytes)?;
        let visible = alpha_weight(&image);
        if best.is_none() || visible > weight {
            best = Some(image);
            weight = visible;
        }
    }
    best.context("The WebM decoder produced no visible-frame candidates")
}

pub async fn refresh(
    stored: &mut BTreeMap<String, Value>,
    entries: &BTreeMap<String, Entry>,
    rebuild: bool,
) -> Refresh {
    refresh_complete(stored, entries, rebuild, true).await
}

pub async fn refresh_complete(
    stored: &mut BTreeMap<String, Value>,
    entries: &BTreeMap<String, Entry>,
    rebuild: bool,
    complete: bool,
) -> Refresh {
    let mut counts = Refresh::default();
    for (id, entry) in entries {
        let Some(path) = entry.thumb.as_ref() else {
            counts.skipped += 1;
            continue;
        };
        if !rebuild
            && stored.get(id).is_some_and(|value| {
                value["stamp"]
                    == entry
                        .stamp
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null)
            })
        {
            update_metadata(
                stored.get_mut(id).expect("Existing stamp was checked"),
                entry,
            );
            counts.unchanged += 1;
            continue;
        }
        let fingerprint = match open_picture(path)
            .await
            .and_then(|picture| vision::fingerprint(&picture))
        {
            Ok(value) => value,
            Err(error) => {
                counts.skipped += 1;
                counts.problems.push(format!("{id}: {error}"));
                continue;
            }
        };
        let mut value = serde_json::to_value(fingerprint)
            .expect("A bounded finite fingerprint is JSON-serializable");
        update_metadata(&mut value, entry);
        if stored.insert(id.clone(), value).is_some() {
            counts.updated += 1;
        } else {
            counts.added += 1;
        }
    }
    stored.retain(|id, value| {
        let keep = !complete || value["src"] != "packs" || entries.contains_key(id);
        if !keep {
            counts.removed += 1;
        }
        keep
    });
    counts
}

fn update_metadata(value: &mut Value, entry: &Entry) {
    for (key, field) in [
        ("src", json!("packs")),
        ("pack", json!(entry.pack)),
        ("title", entry.title.clone()),
        ("link", entry.link.clone()),
        ("glyph", entry.glyph.clone()),
        ("name", entry.name.clone()),
        ("stamp", json!(entry.stamp)),
    ] {
        value[key] = field;
    }
}

pub fn load_cache(path: &Path) -> Result<BTreeMap<String, Value>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_value(read_json(path)?)
        .context("Emoji cache must be an object keyed by document ids")
}

pub fn save_cache(path: &Path, stored: &BTreeMap<String, Value>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temporary.as_file_mut(), stored)?;
    ensure!(
        temporary.as_file().metadata()?.len() <= MAX_JSON_BYTES,
        "Emoji cache exceeds its 64 MiB read/write bound; existing cache was preserved"
    );
    temporary.as_file_mut().sync_all()?;
    crate::state::storage::restrict(temporary.path(), false)?;
    temporary
        .persist(path)
        .context("Could not atomically update the emoji fingerprint cache")?;
    Ok(())
}
