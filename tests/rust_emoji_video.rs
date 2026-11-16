//! Real codec tests keep Telegram offline and isolate process-wide file roots.
use anyhow::{Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use telegram_mcp::backend::Telegram;

struct Wire {
    data: Vec<u8>,
    mime: &'static str,
}
#[async_trait]
impl Telegram for Wire {
    async fn invoke(&self, method: &str, arguments: Value) -> Result<Value> {
        assert_eq!(method, "messages.getCustomEmojiDocuments");
        assert_eq!(arguments["document_id"], json!(["99"]));
        Ok(
            json!([{"_":"document","id":"99","access_hash":"7","file_reference":[],"mime_type":self.mime,"attributes":[{"_":"documentAttributeCustomEmoji","alt":"🔥"}]}]),
        )
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        bail!("No chat resolution is needed for emoji preview")
    }
    async fn download_memory(&self, _: Value, _: u64) -> Result<Vec<u8>> {
        Ok(self.data.clone())
    }
}

fn bounded(mut command: Command) -> std::process::Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(25);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Codec test exceeded its process deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn isolated(name: &str) -> bool {
    if std::env::var("TEST_EMOJI_VIDEO_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env("TEST_EMOJI_VIDEO_CHILD", name)
        .env("TELEGRAM_STATE_DIR", temp.path().join("state"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&[&files]).unwrap(),
        )
        .env("TEST_VIDEO_ROOT", &files);
    let output = bounded(command);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
fn fixture(root: &Path, webm: bool) -> Vec<u8> {
    let source = root.join("frames.rgba");
    let output = root.join(if webm { "emoji.webm" } else { "emoji.mp4" });
    let mut pixels = Vec::new();
    for (index, colour) in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
        .into_iter()
        .enumerate()
    {
        let colour = if webm && index != 1 {
            [0, 0, 0, 0]
        } else {
            colour
        };
        for _ in 0..16 * 16 {
            pixels.extend_from_slice(&colour);
        }
    }
    std::fs::write(&source, pixels).unwrap();
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "16x16",
            "-framerate",
            "3",
            "-i",
        ])
        .arg(source)
        .args(["-frames:v", "3", "-threads", "1"]);
    if webm {
        command.args([
            "-c:v",
            "libvpx-vp9",
            "-lossless",
            "1",
            "-auto-alt-ref",
            "0",
            "-pix_fmt",
            "yuva420p",
        ]);
    } else {
        command.args(["-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p"]);
    }
    command.arg(&output);
    let status = bounded(command);
    assert!(
        status.status.success(),
        "FFmpeg is required for codec CI fixtures: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::read(output).unwrap()
}
async fn preview(wire: &Wire, count: i64) -> Result<Value> {
    telegram_mcp::tools::messaging::call(
        wire,
        "get_custom_emoji",
        &json!({"document_ids":["99"],"max_dimension":64,"count":count}),
    )
    .await
    .unwrap()
}

#[test]
fn video_custom_emoji_returns_individual_alpha_preserving_vp9_frames_with_timestamps() {
    if isolated("video_custom_emoji_returns_individual_alpha_preserving_vp9_frames_with_timestamps")
    {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var("TEST_VIDEO_ROOT").unwrap());
    let wire = Wire {
        data: fixture(&root, true),
        mime: "video/webm",
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let result = preview(&wire, 3).await.unwrap();
        let content = result["content"].as_array().unwrap();
        let meta = result["metadata"]["emoji"][0]["preview"]
            .as_array()
            .unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(meta.len(), 3);
        assert_eq!(meta[0]["blank"], true);
        assert_eq!(meta[1]["blank"], false);
        assert_eq!(meta[2]["blank"], true);
        assert!(meta[2]["timestamp"].as_f64().unwrap() > meta[1]["timestamp"].as_f64().unwrap());
        assert_eq!(meta[1]["source"], "ffmpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content[1]["data"].as_str().unwrap())
            .unwrap();
        let image = telegram_mcp::tools::visual::images::decode_image(&bytes)
            .unwrap()
            .to_rgba8();
        assert!(image.width() <= 64 && image.height() <= 64);
        let pixel = image.get_pixel(image.width() / 2, image.height() / 2);
        assert!(pixel[1] > 220 && pixel[3] == 255);
    });
}

#[test]
fn mp4_emoji_single_frame_is_real_decoded_content_with_its_document_association() {
    if isolated("mp4_emoji_single_frame_is_real_decoded_content_with_its_document_association") {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var("TEST_VIDEO_ROOT").unwrap());
    let wire = Wire {
        data: fixture(&root, false),
        mime: "video/mp4",
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let result = preview(&wire, 1).await.unwrap();
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
        assert_eq!(result["metadata"]["emoji"][0]["document_id"], "99");
        assert_eq!(result["metadata"]["emoji"][0]["placeholder"], "🔥");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result["content"][0]["data"].as_str().unwrap())
            .unwrap();
        let image = telegram_mcp::tools::visual::images::decode_image(&bytes)
            .unwrap()
            .to_rgba8();
        assert!(
            image
                .pixels()
                .all(|pixel| pixel[0] > 200 && pixel[1] < 30 && pixel[2] < 30)
        );
    });
}

#[test]
fn malformed_video_emoji_returns_a_bounded_error_and_removes_temporary_payloads() {
    if isolated("malformed_video_emoji_returns_a_bounded_error_and_removes_temporary_payloads") {
        return;
    }
    let wire = Wire {
        data: b"synthetic-private-codec-payload-not-for-errors".to_vec(),
        mime: "video/webm",
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let result = preview(&wire, 3).await.unwrap();
        assert!(result["content"].as_array().unwrap().is_empty());
        let error = result["metadata"]["emoji"][0]["error"].as_str().unwrap();
        assert!(!error.is_empty() && error.len() <= 200);
        assert!(!error.contains("synthetic-private-codec-payload"));
        assert!(
            !result
                .to_string()
                .contains("synthetic-private-codec-payload")
        );
        let root = std::path::PathBuf::from(std::env::var("TEST_VIDEO_ROOT").unwrap());
        assert_eq!(
            std::fs::read_dir(root).unwrap().count(),
            0,
            "A failed preview must remove all temporary input/frame files"
        );
    });
}
