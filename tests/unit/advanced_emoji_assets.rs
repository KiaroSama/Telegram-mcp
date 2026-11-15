use super::*;
fn fixture() -> (tempfile::TempDir, State) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = State::at(temp.path().join("state"), vec![files]).unwrap();
    (temp, state)
}
#[test]
fn emoji_output_is_published_without_replacing_existing_files() {
    let (_temp, state) = fixture();
    let path = state.file_roots()[0].join("emoji.png");
    let first = save(&state, &path, b"first", 42).unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), b"first");
    assert!(save(&state, &path, b"second", 42).is_err());
    assert_eq!(std::fs::read(&first).unwrap(), b"first");
}
#[test]
fn emoji_output_rejects_paths_outside_allowed_roots() {
    let (temp, state) = fixture();
    assert!(save(&state, &temp.path().join("outside.png"), b"fixture", 42).is_err());
    assert!(!temp.path().join("outside.png").exists());
}
#[test]
fn directory_output_uses_sniffed_format_and_exact_document_id() {
    let (_temp, state) = fixture();
    let path = save(
        &state,
        &state.file_roots()[0],
        b"\x89PNGfixture",
        1234567890123456789,
    )
    .unwrap();
    assert_eq!(
        path.file_name().unwrap(),
        "custom_emoji_1234567890123456789.png"
    );
}
#[test]
fn emoji_suffix_uses_actual_bytes_instead_of_advertised_mime() {
    assert_eq!(suffix(b"\x1f\x8bcompressed"), "tgs");
    assert_eq!(suffix(b"RIFF1234WEBPpayload"), "webp");
    assert_eq!(suffix(b"\x1aE\xdf\xa3video"), "webm");
    assert_eq!(suffix(b"unknown"), "bin");
}
#[test]
fn saved_emoji_bytes_are_complete_and_unix_permissions_are_private() {
    let (_temp, state) = fixture();
    let path = save(
        &state,
        &state.file_roots()[0].join("asset.bin"),
        b"fixture",
        42,
    )
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

struct MediaApi {
    files: std::sync::Mutex<std::collections::VecDeque<std::result::Result<Vec<u8>, &'static str>>>,
    reads: std::sync::Mutex<Vec<(Value, u64)>>,
}
impl MediaApi {
    fn new(files: Vec<std::result::Result<Vec<u8>, &'static str>>) -> Self {
        Self {
            files: std::sync::Mutex::new(files.into()),
            reads: std::sync::Mutex::new(vec![]),
        }
    }
}
#[async_trait::async_trait]
impl Telegram for MediaApi {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        anyhow::bail!("No RPC expected")
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        anyhow::bail!("No peer lookup expected")
    }
    async fn download_memory(&self, location: Value, max: u64) -> Result<Vec<u8>> {
        self.reads.lock().unwrap().push((location, max));
        self.files
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected read")
            .map_err(|error| anyhow::anyhow!(error))
    }
}
fn document() -> Value {
    json!({"_":"document","id":"42","access_hash":"99","file_reference":[1],"dc_id":4})
}

#[tokio::test]
async fn remote_emoji_thumbnail_preserves_its_actual_location_and_byte_budget() {
    let mut doc = document();
    doc["thumbs"] = json!([{"_":"photoSizeEmpty","type":"x"},{"_":"photoSize","type":"m","w":64,"h":64,"size":3}]);
    let api = MediaApi::new(vec![Ok(vec![1, 2, 3])]);
    let result = bytes(&api, &doc, true, 4).await.unwrap();
    assert_eq!(result, (vec![1, 2, 3], true));
    let reads = api.reads.lock().unwrap();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].0["thumb_size"], "m");
    assert_eq!(reads[0].0["_dc_id"], 4);
    assert_eq!(reads[0].1, 4);
}

#[tokio::test]
async fn missing_or_empty_thumbnail_falls_back_to_original_bytes_with_same_budget() {
    for failure in [Err("THUMB_MISSING"), Ok(vec![])] {
        let mut doc = document();
        doc["thumbs"] = json!([{"_":"photoSize","type":"m","w":64,"h":64,"size":3}]);
        let api = MediaApi::new(vec![failure, Ok(vec![9, 8, 7])]);
        assert_eq!(
            bytes(&api, &doc, true, 4).await.unwrap(),
            (vec![9, 8, 7], false)
        );
        let reads = api.reads.lock().unwrap();
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[1].0["thumb_size"], "");
        assert!(reads.iter().all(|(_, cap)| *cap == 4));
    }
}

#[tokio::test]
async fn corrupt_inline_thumbnail_falls_back_but_oversized_inline_bytes_fail_closed() {
    let mut doc = document();
    doc["thumbs"] = json!([{"_":"photoStrippedSize","type":"i","bytes":[0]}]);
    let api = MediaApi::new(vec![Ok(vec![9])]);
    assert_eq!(bytes(&api, &doc, true, 4).await.unwrap(), (vec![9], false));
    doc["thumbs"] = json!([{"_":"photoCachedSize","type":"s","w":1,"h":1,"bytes":[1,2,3]}]);
    let api = MediaApi::new(vec![]);
    assert!(
        bytes(&api, &doc, true, 2)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeds max_bytes")
    );
    assert!(api.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn original_emoji_download_rejects_empty_or_over_budget_data() {
    let api = MediaApi::new(vec![Ok(vec![])]);
    assert!(
        bytes(&api, &document(), false, 4)
            .await
            .unwrap_err()
            .to_string()
            .contains("empty custom emoji")
    );
    let api = MediaApi::new(vec![Ok(vec![1, 2, 3])]);
    assert!(
        bytes(&api, &document(), false, 2)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeded max_bytes")
    );
}
