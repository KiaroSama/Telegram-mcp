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
