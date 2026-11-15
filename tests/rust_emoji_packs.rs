use image::{DynamicImage, Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use telegram_mcp::emoji::{accuracy, packs, vision};

fn thumb(path: &Path, colour: [u8; 4], size: u32) {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(size, size, Rgba(colour)))
        .write_to(
            &mut std::fs::File::create(path).unwrap(),
            image::ImageFormat::WebP,
        )
        .unwrap();
}
fn export(root: &Path, items: &[(&str, &str)]) {
    std::fs::create_dir_all(root.join(".thumbs")).unwrap();
    std::fs::write(root.join("index.json"),serde_json::to_vec(&json!({"packs":[{"set_name":"p","title":"بسته","family":"general","link":"https://t.me/addemoji/p"}]})).unwrap()).unwrap();
    let emoji: Vec<Value> = items
        .iter()
        .map(|(id, glyph)| json!({"custom_emoji_id":id,"glyph":glyph,"role":"emoji","name":null}))
        .collect();
    std::fs::write(
        root.join("p.json"),
        serde_json::to_vec(&json!({"emoji":emoji})).unwrap(),
    )
    .unwrap();
    for (id, _) in items {
        thumb(
            &root.join(format!(".thumbs/{id}.webp")),
            [255, 0, 0, 255],
            8,
        );
    }
}

#[test]
fn packs_root_requires_a_real_export_index() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        packs::find_root(Some(temp.path()))
            .unwrap_err()
            .to_string()
            .contains("not an Emoji Mapper export")
    );
}
#[test]
fn catalogue_joins_pack_metadata_and_preserves_utf8() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "💳")]);
    let (entries, problems) = packs::read_catalogue(temp.path()).unwrap();
    assert!(problems.is_empty());
    assert_eq!(entries["1"].title, "بسته");
    assert_eq!(entries["1"].glyph, "💳");
    assert_eq!(entries["1"].link, "https://t.me/addemoji/p");
    assert!(entries["1"].stamp.is_some());
}
#[test]
fn catalogue_numeric_long_ids_never_round_through_float() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("6325520979057445692", "✅")]);
    let detail = json!({"emoji":[{"custom_emoji_id":6325520979057445692_i64,"glyph":"✅"}]});
    std::fs::write(
        temp.path().join("p.json"),
        serde_json::to_vec(&detail).unwrap(),
    )
    .unwrap();
    assert!(
        packs::read_catalogue(temp.path())
            .unwrap()
            .0
            .contains_key("6325520979057445692")
    );
}
#[test]
fn catalogue_reports_a_missing_pack_without_discarding_good_entries() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    let mut index = packs::read_json(&temp.path().join("index.json")).unwrap();
    index["packs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"set_name":"missing"}));
    std::fs::write(
        temp.path().join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    let (entries, problems) = packs::read_catalogue(temp.path()).unwrap();
    assert!(entries.contains_key("1"));
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("missing"));
}
#[test]
fn catalogue_refuses_pack_path_traversal() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[]);
    std::fs::write(
        temp.path().join("index.json"),
        serde_json::to_vec(&json!({"packs":[{"set_name":"../secret"}]})).unwrap(),
    )
    .unwrap();
    let (entries, problems) = packs::read_catalogue(temp.path()).unwrap();
    assert!(entries.is_empty());
    assert!(problems[0].contains("unsafe"));
}
#[test]
fn thumb_ids_cannot_escape_the_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    assert!(packs::thumb_for(temp.path(), "../secret").is_err());
}
#[test]
fn animated_thumb_is_preferred_over_an_exported_still() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    thumb(&temp.path().join(".thumbs/1_still.webp"), [0, 0, 0, 0], 8);
    assert!(
        packs::thumb_for(temp.path(), "1")
            .unwrap()
            .unwrap()
            .ends_with("1.webp")
    );
}
#[test]
fn still_thumb_remains_a_fallback_when_animation_disappears() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    std::fs::remove_file(temp.path().join(".thumbs/1.webp")).unwrap();
    thumb(
        &temp.path().join(".thumbs/1_still.webp"),
        [255, 0, 0, 255],
        8,
    );
    assert!(
        packs::thumb_for(temp.path(), "1")
            .unwrap()
            .unwrap()
            .ends_with("1_still.webp")
    );
}
#[test]
fn webm_only_thumb_is_found_with_its_real_suffix() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    std::fs::remove_file(temp.path().join(".thumbs/1.webp")).unwrap();
    std::fs::write(temp.path().join(".thumbs/1.webm"), b"video").unwrap();
    assert!(
        packs::thumb_for(temp.path(), "1")
            .unwrap()
            .unwrap()
            .ends_with("1.webm")
    );
}
#[test]
fn source_id_table_provides_an_exact_counterpart_before_visual_ranking() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("999", "🛒")]);
    let mut index = packs::read_json(&temp.path().join("index.json")).unwrap();
    index["by_source_id"] = json!({"4967523779728114410":999});
    std::fs::write(
        temp.path().join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    assert_eq!(
        packs::source_map(temp.path()).unwrap()["4967523779728114410"],
        "999"
    );
}
#[test]
fn missing_source_table_is_unknown_not_a_false_no_equivalent_answer() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    assert!(packs::source_map(temp.path()).unwrap().is_empty());
}

#[tokio::test]
async fn refresh_first_pass_fingerprints_and_second_pass_only_stats() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a"), ("2", "b")]);
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let mut stored = BTreeMap::new();
    let first = packs::refresh(&mut stored, &entries, false).await;
    assert_eq!(first.added, 2);
    let snapshot = stored.clone();
    let second = packs::refresh(&mut stored, &entries, false).await;
    assert_eq!(second.unchanged, 2);
    assert_eq!(
        second.added + second.updated + second.removed + second.skipped,
        0
    );
    assert_eq!(stored, snapshot);
}
#[tokio::test]
async fn refresh_uses_changed_thumbnail_not_a_stale_fingerprint() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let mut stored = BTreeMap::new();
    packs::refresh(&mut stored, &entries, false).await;
    let before = stored["1"].clone();
    let path = temp.path().join(".thumbs/1.webp");
    thumb(&path, [0, 0, 255, 255], 16);
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1))
        .unwrap();
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let result = packs::refresh(&mut stored, &entries, false).await;
    assert_eq!(result.updated, 1);
    assert_ne!(stored["1"]["colour"], before["colour"]);
}
#[tokio::test]
async fn refresh_forgets_only_vanished_pack_owned_entries() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a"), ("2", "b")]);
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let mut stored = BTreeMap::from([(
        "other".into(),
        json!({"pack":"someone_else","silhouette":[0.1]}),
    )]);
    packs::refresh(&mut stored, &entries, false).await;
    export(temp.path(), &[("1", "a")]);
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let result = packs::refresh(&mut stored, &entries, false).await;
    assert_eq!(result.removed, 1);
    assert!(!stored.contains_key("2"));
    assert!(stored.contains_key("other"));
}
#[tokio::test]
async fn refresh_reports_unreadable_thumbnails_and_keeps_good_ones() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a"), ("2", "b")]);
    std::fs::write(temp.path().join(".thumbs/2.webp"), b"truncated").unwrap();
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let mut stored = BTreeMap::new();
    let result = packs::refresh(&mut stored, &entries, false).await;
    assert_eq!((result.added, result.skipped), (1, 1));
    assert!(result.problems[0].contains('2'));
}
#[tokio::test]
async fn rebuild_recomputes_pack_entries_and_preserves_remote_ones() {
    let temp = tempfile::tempdir().unwrap();
    export(temp.path(), &[("1", "a")]);
    let (entries, _) = packs::read_catalogue(temp.path()).unwrap();
    let mut stored = BTreeMap::from([("remote".into(), json!({"pack":"other"}))]);
    packs::refresh(&mut stored, &entries, false).await;
    let result = packs::refresh(&mut stored, &entries, true).await;
    assert_eq!(result.added, 1);
    assert_eq!(result.unchanged, 0);
    assert!(stored.contains_key("remote"));
}
#[test]
fn video_thumb_names_vp9_decoder_before_input_to_preserve_alpha() {
    let args = packs::video_arguments(Path::new("input.webm"), Path::new("%03d.png"));
    let args: Vec<_> = args.iter().map(|s| s.to_string_lossy()).collect();
    let decoder = args.iter().position(|arg| arg == "-c:v").unwrap();
    let input = args.iter().position(|arg| arg == "-i").unwrap();
    assert!(decoder < input);
    assert_eq!(args[decoder + 1], "libvpx-vp9");
    assert!(args.iter().any(|arg| arg == "-max_pixels"));
}
#[test]
fn cache_round_trip_preserves_metadata_floats_and_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cache.json");
    let mut value =
        serde_json::to_value(vision::fingerprint(&DynamicImage::new_rgba8(8, 8)).unwrap()).unwrap();
    value["title"] = "بسته 😀".into();
    let stored = BTreeMap::from([("9007199254740993".into(), value)]);
    packs::save_cache(&path, &stored).unwrap();
    assert_eq!(packs::load_cache(&path).unwrap(), stored);
}

#[test]
fn accuracy_reads_true_catalog_pairs_without_modifying_the_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("create table items(custom_emoji_id text,keywords text)")
            .unwrap();
        db.execute(
            "insert into items values(?1,?2)",
            rusqlite::params![
                "9007199254740993",
                r#"["premium-id:7000000000000001","search:cart","premium-id:9007199254740993"]"#
            ],
        )
        .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let pairs = accuracy::known_pairs(&path).unwrap();
    assert_eq!(pairs["7000000000000001"], "9007199254740993");
    assert!(!pairs.contains_key("9007199254740993"));
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[test]
fn accuracy_reports_unresolved_originals_instead_of_inflating_hit_rates() {
    let red = vision::fingerprint(&DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        8,
        8,
        Rgba([255, 0, 0, 255]),
    )))
    .unwrap();
    let blue = vision::fingerprint(&DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        8,
        8,
        Rgba([0, 0, 255, 255]),
    )))
    .unwrap();
    let index = BTreeMap::from([
        ("own_red".into(), serde_json::to_value(&red).unwrap()),
        ("own_blue".into(), serde_json::to_value(&blue).unwrap()),
    ]);
    let pairs = BTreeMap::from([
        ("source_red".into(), "own_red".into()),
        ("missing_original".into(), "own_blue".into()),
    ]);
    let report = accuracy::measure(
        &pairs,
        &BTreeMap::from([("source_red".into(), red)]),
        &index,
    );
    assert_eq!(report.attempted, 2);
    assert_eq!(report.resolved, 1);
    assert_eq!(report.top_1, 1);
    assert_eq!(report.unresolved, vec!["missing_original"]);
    assert_eq!(report.median_rank, Some(1.0));
}
