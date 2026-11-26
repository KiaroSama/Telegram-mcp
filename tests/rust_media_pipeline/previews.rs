use crate::support::*;
use serde_json::{Value, json};
use telegram_mcp::tools::messaging::media;
async fn run(w: &MediaWire, name: &str, a: Value) -> anyhow::Result<Value> {
    media::call(w, name, &a).await.unwrap()
}
#[tokio::test]
async fn server_thumbnail_download_uses_the_selected_declared_type() {
    let mut doc = document(7, "thumb.bin", 100);
    doc["thumbs"] = json!([{"_":"photoSize","type":"m","w":32,"h":16,"size":80},{"_":"photoSize","type":"x","w":64,"h":32,"size":120}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        png(),
    );
    let result = run(
        &w,
        "get_media_thumbnail",
        json!({"chat_id":7,"message_id":8,"thumb_index":0,"max_bytes":4096}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["thumb_size"], "m");
    assert_eq!(result["metadata"]["kind"], "thumbnail");
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn inline_cached_thumbnails_render_without_any_download() {
    let mut doc = document(7, "thumb.bin", 100);
    doc["thumbs"] = json!([{"_":"photoCachedSize","type":"m","w":32,"h":16,"bytes":png()}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        vec![],
    );
    let result = run(
        &w,
        "get_media_thumbnail",
        json!({"chat_id":7,"message_id":8}),
    )
    .await
    .unwrap();
    assert_eq!(result["content"][0]["type"], "image");
    assert!(w.downloads.lock().unwrap().is_empty());
}
#[tokio::test]
async fn thumbnail_selection_rejects_out_of_range_or_missing_declared_sizes() {
    for thumbs in [
        json!([]),
        json!([{"_":"photoSize","type":"m","w":1,"h":1,"size":1}]),
    ] {
        let mut doc = document(7, "a.bin", 1);
        doc["thumbs"] = thumbs;
        let w = MediaWire::new(
            vec![(
                "messages.getMessages",
                media_message(json!({"document":doc})),
            )],
            vec![],
        );
        assert!(
            run(
                &w,
                "get_media_thumbnail",
                json!({"chat_id":7,"message_id":8,"thumb_index":8})
            )
            .await
            .is_err()
        );
        assert!(w.downloads.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn premium_effect_frames_select_video_thumb_f_instead_of_the_sticker_document() {
    let mut doc = document(7, "sticker.webm", 100);
    doc["video_thumbs"] = json!([{"_":"videoSize","type":"f","w":32,"h":16,"size":100}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        png(),
    );
    let result = run(
        &w,
        "get_media_frames",
        json!({"chat_id":7,"message_id":8,"premium_effect":true}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["thumb_size"], "f");
    assert_eq!(result["metadata"]["static"], true);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":document(7,"plain.webm",1)})),
        )],
        png(),
    );
    assert!(
        run(
            &w,
            "get_media_frames",
            json!({"chat_id":7,"message_id":8,"premium_effect":true})
        )
        .await
        .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
}
#[tokio::test]
async fn custom_emoji_id_and_preview_budget_validation_precedes_the_network() {
    for args in [
        json!({"document_ids":[]}),
        json!({"document_ids":["0"]}),
        json!({"document_ids":["9223372036854775808"]}),
        json!({"document_ids":vec!["3";25]}),
        json!({"document_ids":["3"],"max_bytes":0}),
    ] {
        let w = MediaWire::new(vec![], vec![]);
        assert!(run(&w, "get_custom_emoji", args).await.is_err());
        assert!(w.calls().is_empty());
    }
    for response in [json!({}), json!([])] {
        let w = MediaWire::new(vec![("messages.getCustomEmojiDocuments", response)], vec![]);
        assert!(
            run(&w, "get_custom_emoji", json!({"document_ids":["3"]}))
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn invalid_thumbnail_pixels_and_transfer_budgets_return_honest_errors() {
    let mut doc = document(7, "a.bin", 1);
    doc["thumbs"] = json!([{"_":"photoSize","type":"m","w":1,"h":1,"size":1}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        vec![1, 2, 3],
    );
    assert!(
        run(
            &w,
            "get_media_thumbnail",
            json!({"chat_id":7,"message_id":8})
        )
        .await
        .is_err()
    );
    let w = MediaWire::new(vec![], vec![]);
    assert!(media::download_bytes(&w, json!({}), 0).await.is_err());
    assert!(
        media::download_bytes(&w, json!({}), 512 * 1024 * 1024 + 1)
            .await
            .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_oversized_custom_emoji_does_not_discard_other_valid_previews() {
    let mut big = document(3, "large.webp", 5000);
    big["mime_type"] = json!("image/webp");
    let mut good = document(4, "small.png", 100);
    good["mime_type"] = json!("image/png");
    let w = MediaWire::new(
        vec![("messages.getCustomEmojiDocuments", json!([big, good]))],
        png(),
    );
    let result = run(
        &w,
        "get_custom_emoji",
        json!({"document_ids":["3","4"],"max_bytes":4096}),
    )
    .await
    .unwrap();
    assert!(result["metadata"]["emoji"][0]["error"].is_string());
    assert_eq!(result["metadata"]["emoji"][1]["document_id"], "4");
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(w.downloads.lock().unwrap().len(), 1);
}

#[test]
fn video_frames_sample_the_full_clip_instead_of_only_its_opening_seconds() {
    use std::{ffi::OsStr, process::Stdio};
    let name = "rust_media_pipeline::previews::video_frames_sample_the_full_clip_instead_of_only_its_opening_seconds";
    if std::env::var("TEST_NATIVE_VIDEO_SPAN").as_deref() != Ok("child") {
        let (_dir, state) = crate::local_support::fixture();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TEST_NATIVE_VIDEO_SPAN", "child")
            .env("TELEGRAM_STATE_DIR", state.root())
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(state.file_roots()).unwrap(),
            )
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("span.mp4");
    ffmpeg(&[
        OsStr::new("-f"),
        OsStr::new("lavfi"),
        OsStr::new("-i"),
        OsStr::new("color=c=red:s=32x32:r=1:d=3"),
        OsStr::new("-f"),
        OsStr::new("lavfi"),
        OsStr::new("-i"),
        OsStr::new("color=c=blue:s=32x32:r=1:d=3"),
        OsStr::new("-filter_complex"),
        OsStr::new("[0:v][1:v]concat=n=2:v=1:a=0"),
        OsStr::new("-c:v"),
        OsStr::new("mpeg4"),
        path.as_os_str(),
    ]);
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(media::previews::render(
            &std::fs::read(path).unwrap(),
            2,
            128,
        ))
        .unwrap();
    assert!(
        result["metadata"]["frame_metadata"][1]["timestamp"]
            .as_f64()
            .unwrap_or(0.0)
            >= 3.0,
        "The second still must sample the later blue half"
    );
    use base64::Engine;
    let pixels = base64::engine::general_purpose::STANDARD
        .decode(result["content"][0]["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&pixels).unwrap().to_rgb8();
    assert!(
        image.pixels().any(|p| p[2] > 150 && p[0] < 70),
        "The actual contact sheet must include blue pixels from the latter half"
    );
}

#[tokio::test]
async fn custom_emoji_from_same_set_share_one_metadata_request() {
    let mut d = document(71, "icon.webp", 4);
    d["mime_type"] = json!("image/webp");
    d["attributes"] = json!([{"_":"documentAttributeCustomEmoji","alt":"x","stickerset":{"_":"inputStickerSetID","id":"17","access_hash":"1"}}]);
    let mut e = d.clone();
    e["id"] = json!("72");
    let w = MediaWire::new(
        vec![
            ("messages.getCustomEmojiDocuments", json!([d, e])),
            (
                "messages.getStickerSet",
                json!({"set":{"short_name":"ExampleSet","title":"Example set"}}),
            ),
        ],
        png(),
    );
    let result = run(&w, "get_custom_emoji", json!({"document_ids":[71,72]}))
        .await
        .unwrap();
    assert_eq!(
        result["metadata"]["emoji"][0]["set_short_name"],
        "ExampleSet"
    );
    assert_eq!(result["metadata"]["emoji"][1]["set_title"], "Example set");
    assert_eq!(
        w.calls()
            .iter()
            .filter(|(name, _)| name == "messages.getStickerSet")
            .count(),
        1
    );
}
