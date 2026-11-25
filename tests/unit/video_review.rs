use super::*;

#[test]
fn protected_disappearing_and_nonvideo_media_are_rejected() {
    let mut message = json!({"media":{"document":{"attributes":[{"_":"documentAttributeVideo"}]}}});
    assert!(validate_media(&message).is_ok());
    message["noforwards"] = json!(true);
    assert!(
        validate_media(&message)
            .unwrap_err()
            .to_string()
            .contains("protected")
    );
    message["noforwards"] = json!(false);
    message["media"]["ttl_seconds"] = json!(1);
    assert!(
        validate_media(&message)
            .unwrap_err()
            .to_string()
            .contains("disappearing")
    );
    assert!(validate_media(&json!({"media":{"document":{"attributes":[]}}})).is_err());
}

#[test]
fn video_window_returns_bounded_frame_positions_and_validates_numbers() {
    assert_eq!(
        seconds(&json!({"start":1.25}), "start").unwrap(),
        Some(1.25)
    );
    assert!(seconds(&json!({"start":-1}), "start").is_err());
    assert!(seconds(&json!({"start":"1"}), "start").is_err());
    assert_eq!(
        emoji_previews::frame_times(2.0, 6.0, 3),
        vec![2.0, 4.0, 6.0]
    );
    assert_eq!(emoji_previews::frame_times(2.0, 6.0, 1), vec![2.0]);
}

#[tokio::test]
async fn video_decodes_one_silent_download_and_removes_private_work_files() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct VideoApi {
        bytes: Vec<u8>,
        downloads: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Telegram for VideoApi {
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            Ok(json!({"_":"inputPeerUser","user_id":"7","access_hash":"9"}))
        }
        async fn invoke(&self, name: &str, _: Value) -> Result<Value> {
            assert_eq!(name, "messages.getMessages");
            Ok(
                json!({"messages":[{"_":"message","id":8,"peer_id":{"_":"peerUser","user_id":"7"},
                "media":{"document":{"_":"document","id":"6","access_hash":"9","file_reference":[],"dc_id":2,
                "size":self.bytes.len().to_string(),"attributes":[{"_":"documentAttributeVideo"}]}}}]}),
            )
        }
        async fn download(&self, _: Value, destination: &Path, max: u64) -> Result<Value> {
            self.downloads.fetch_add(1, Ordering::Relaxed);
            assert!(self.bytes.len() as u64 <= max);
            tokio::fs::write(destination, &self.bytes).await?;
            Ok(Value::Null)
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let fixture = temp.path().join("fixture.mp4");
    let state = State::at(temp.path().join("state"), vec![temp.path().to_owned()]).unwrap();
    let status = tokio::time::timeout(
        Duration::from_secs(8),
        decoder()
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=64x64:r=10:d=0.4",
                "-c:v",
                "mpeg4",
            ])
            .arg(&fixture)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(status.success());
    let api = VideoApi {
        bytes: std::fs::read(&fixture).unwrap(),
        downloads: AtomicUsize::new(0),
    };
    let result = view_in(
        &api,
        &state,
        &json!({"chat_id":7,"message_id":8,"max_frames":2}),
    )
    .await
    .unwrap();
    assert_eq!(result["transcript"]["status"], "no_audio");
    assert_eq!(result["sampling"], "even_intervals");
    assert_eq!(result["frame_count"], 2);
    assert_eq!(result["content"][1]["mimeType"], "image/png");
    assert_eq!(api.downloads.load(Ordering::Relaxed), 1);
    assert!(
        !std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".telegram-video-"))
    );
}
