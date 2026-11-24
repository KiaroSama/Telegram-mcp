use super::*;
use std::io::Cursor;
#[test]
fn chunk_boundary_does_not_hide_a_real_credential() {
    let credential = ["ghp_", &"a".repeat(36)].concat();
    let mut bytes = vec![b' '; CHUNK - 8];
    bytes.extend_from_slice(credential.as_bytes());
    bytes.push(b' ');
    let size = bytes.len() as u64;
    let found = body(
        &mut Cursor::new(bytes),
        size,
        true,
        &rules().unwrap(),
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert!(found.contains("GitHub token"));
}
#[test]
fn chunk_boundary_cannot_invent_a_word_boundary() {
    let candidate = ["AKIA", &"A".repeat(16)].concat();
    let mut bytes = vec![b' '; CHUNK - candidate.len()];
    bytes.extend_from_slice(candidate.as_bytes());
    bytes.push(b'A');
    let size = bytes.len() as u64;
    assert!(
        body(
            &mut Cursor::new(bytes),
            size,
            true,
            &rules().unwrap(),
            Instant::now() + Duration::from_secs(5)
        )
        .unwrap()
        .is_empty()
    );
}
#[test]
fn truncated_or_unterminated_object_is_not_a_clean_scan() {
    let id = "ab".repeat(20);
    let expected = BTreeSet::from([id.clone()]);
    for report in [format!("{id} blob 9\nshort"), format!("{id} blob 1\nx?")] {
        assert!(
            stream(
                &mut Cursor::new(report),
                &expected,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
    }
}
#[test]
fn early_eof_with_a_missing_reachable_object_is_refused() {
    let id = "ab".repeat(20);
    let other = "cd".repeat(20);
    let report = format!("{id} blob 1\nx\n");
    assert!(
        stream(
            &mut Cursor::new(report),
            &BTreeSet::from([id, other]),
            Instant::now() + Duration::from_secs(5)
        )
        .is_err()
    );
}
#[test]
fn malformed_and_oversized_headers_are_refused() {
    assert!(
        stream(
            &mut Cursor::new(vec![b'x'; 4097]),
            &BTreeSet::new(),
            Instant::now() + Duration::from_secs(5)
        )
        .is_err()
    );
    let id = "ab".repeat(20);
    let report = format!("{id} blob {}\n", MAX_OBJECT + 1);
    assert!(
        stream(
            &mut Cursor::new(report),
            &BTreeSet::from([id]),
            Instant::now() + Duration::from_secs(5)
        )
        .is_err()
    );
}
