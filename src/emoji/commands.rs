use super::cli::{Cli, PackSource, cache, ids, source, tail};
use super::{accuracy, packs, sheet, studio::Studio, vision};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
pub(super) async fn index(
    cli: &Cli,
    source: &PackSource,
    remote: Option<&str>,
    size: u32,
    frames: usize,
    rebuild: bool,
) -> Result<()> {
    let path = cache(cli)?;
    let mut stored = packs::load_cache(&path)?;
    if source.packs_dir.is_some() || remote.is_none() {
        let root = packs::find_root(source.packs_dir.as_deref())?;
        let (entries, problems) = packs::read_catalogue(&root)?;
        let complete = problems.is_empty();
        for problem in problems {
            eprintln!("packs: {problem}");
        }
        let counts = packs::refresh_complete(&mut stored, &entries, rebuild, complete).await;
        println!("{}", serde_json::to_string_pretty(&counts)?);
    } else {
        let studio = Studio::connect(cli.account.clone(), cli.url.as_deref()).await?;
        for name in remote
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let ids = studio.pack_ids(name).await?;
            let fresh: Vec<String> = ids
                .into_iter()
                .filter(|id| rebuild || !stored.contains_key(id))
                .collect();
            for chunk in fresh.chunks(10) {
                for (id, picture) in studio.images(chunk, size, frames).await? {
                    let mut fingerprint = serde_json::to_value(vision::fingerprint(&picture)?)?;
                    fingerprint["pack"] = name.into();
                    stored.insert(id, fingerprint);
                }
            }
        }
    }
    packs::save_cache(&path, &stored)?;
    println!("{} fingerprints -> {}", stored.len(), path.display());
    Ok(())
}

pub(super) async fn nearest(
    cli: &Cli,
    options: &PackSource,
    id: &str,
    k: usize,
    size: u32,
    out: &Path,
) -> Result<()> {
    ids(id)?;
    ensure!((1..=511).contains(&k), "k must be between 1 and 511");
    let path = cache(cli)?;
    ensure!(path.exists(), "No fingerprint cache; run index first");
    let stored = packs::load_cache(&path)?;
    ensure!(
        !stored.is_empty(),
        "The fingerprint cache is empty; run index first"
    );
    let mut source = source(cli, options)?;
    if let Some(exact) = source.from_source.get(id) {
        println!("EXACT  {id} was copied into {exact}");
        if let Some(entry) = source.entries.get(exact) {
            println!(
                "{} {}",
                entry.title.as_str().unwrap_or(""),
                entry.link.as_str().unwrap_or("")
            );
        }
    }
    let mut pictures = source.get(&[id.to_owned()], size).await?;
    let query = pictures
        .get(id)
        .context("No picture was returned for the query emoji")?;
    let target = vision::fingerprint(query)?;
    let ranked = vision::rank(&target, &stored, k, &BTreeSet::from([id.to_owned()]));
    let top: Vec<String> = ranked.iter().map(|(_, id)| id.clone()).collect();
    pictures.extend(source.get(&top, size).await?);
    let order: Vec<String> = std::iter::once(id.to_owned()).chain(top).collect();
    let mut labels = BTreeMap::from([(id.to_owned(), "QUERY".into())]);
    for (score, key) in &ranked {
        labels.insert(key.clone(), format!("{score:.3} {}", tail(key)));
        let fingerprint = serde_json::from_value(stored[key].clone()).unwrap_or_default();
        let parts = vision::distance(&target, &fingerprint);
        println!(
            "{score:.4}  {key}  {} ({})  silhouette={:.3} colour={:.3} structure={:.3}",
            stored[key]["glyph"].as_str().unwrap_or(""),
            stored[key]["title"]
                .as_str()
                .or_else(|| stored[key]["pack"].as_str())
                .unwrap_or("?"),
            parts.silhouette,
            parts.colour,
            parts.structure
        );
        if let Some(link) = stored[key]["link"].as_str() {
            println!("          {link}");
        }
    }
    sheet::contact_sheet(&pictures, &order, &labels, order.len().min(16), 84, out)?;
    println!("{}", out.display());
    Ok(())
}

pub(super) async fn gif(
    cli: &Cli,
    id: &str,
    out: &Path,
    size: u32,
    count: usize,
    duration: u32,
) -> Result<()> {
    ids(id)?;
    ensure!(
        (1..=10).contains(&count) && (1..=10000).contains(&duration),
        "frames must be 1-10 and duration 1-10000 ms"
    );
    let studio = Studio::connect(cli.account.clone(), cli.url.as_deref()).await?;
    let (mut images, _) = studio.frames(&[id.to_owned()], size, count).await?;
    let frames = images
        .remove(id)
        .context("No preview was returned for this emoji")?;
    ensure!(
        frames.len() >= 2,
        "The emoji rendered fewer than two actual frames; it is not animated"
    );
    let parent = out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(file.as_file_mut());
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite)?;
        for (data, _) in &frames {
            let image = crate::tools::visual::images::decode_image(data)?;
            let mut flat = image::RgbaImage::from_pixel(
                image.width(),
                image.height(),
                image::Rgba([255, 255, 255, 255]),
            );
            image::imageops::overlay(&mut flat, &image.to_rgba8(), 0, 0);
            encoder.encode_frame(image::Frame::from_parts(
                flat,
                0,
                0,
                image::Delay::from_numer_denom_ms(duration, 1),
            ))?;
        }
    }
    file.as_file_mut().sync_all()?;
    file.persist(out)?;
    println!("{}  ({} frames)", out.display(), frames.len());
    Ok(())
}

pub(super) async fn measure_accuracy(
    cli: &Cli,
    catalog: &Path,
    sample: usize,
    size: u32,
) -> Result<()> {
    ensure!(
        (1..=10000).contains(&sample),
        "sample must be between 1 and 10000"
    );
    let path = cache(cli)?;
    ensure!(path.exists(), "No fingerprint cache; run index first");
    let stored = packs::load_cache(&path)?;
    let pairs: BTreeMap<String, String> = accuracy::known_pairs(catalog)?
        .into_iter()
        .filter(|(_, wanted)| stored.contains_key(wanted))
        .take(sample)
        .collect();
    let studio = Studio::connect(cli.account.clone(), cli.url.as_deref()).await?;
    let mut queries = BTreeMap::new();
    let ids: Vec<String> = pairs.keys().cloned().collect();
    for chunk in ids.chunks(10) {
        match studio.images(chunk, size, 3).await {
            Ok(images) => {
                for (id, image) in images {
                    queries.insert(id, vision::fingerprint(&image)?);
                }
            }
            Err(error) => eprintln!("Unresolved accuracy batch: {error}"),
        }
    }
    let report = accuracy::measure(&pairs, &queries, &stored);
    println!("{}", serde_json::to_string_pretty(&report)?);
    for (cut, hits) in [
        (1, report.top_1),
        (3, report.top_3),
        (8, report.top_8),
        (20, report.top_20),
    ] {
        println!(
            "top-{cut:<2}: {hits}/{} ({:.1}%)",
            report.attempted,
            if report.attempted == 0 {
                0.0
            } else {
                100.0 * hits as f64 / report.attempted as f64
            }
        );
    }
    Ok(())
}
