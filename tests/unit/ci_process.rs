use super::*;
use std::io::Cursor;

#[test]
fn subprocess_output_keeps_exact_bytes_at_the_cap_and_refuses_one_extra_byte() {
    assert_eq!(read_bounded(Cursor::new(b"exact"), 5).unwrap(), b"exact");
    assert!(read_bounded(Cursor::new(b"extra!"), 5).is_err());
    assert!(read_bounded(Cursor::new(b"x"), 0).is_err());
    assert!(read_bounded(Cursor::new(b""), 0).unwrap().is_empty());
}

#[test]
fn subprocess_output_refusal_does_not_include_private_child_bytes() {
    let error = read_bounded(Cursor::new(b"private-child-content"), 4).unwrap_err();
    assert!(!error.to_string().contains("private-child-content"));
}

#[test]
fn inherited_pipe_cannot_outlive_the_shared_subprocess_deadline() {
    let (_held_sender, receiver) = mpsc::sync_channel(1);
    let started = Instant::now();
    assert!(receive_output(receiver, started + Duration::from_millis(20)).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn subprocess_reader_error_is_not_converted_into_empty_success() {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender
        .send(Err(std::io::Error::other("synthetic-read-failure")))
        .unwrap();
    assert!(receive_output(receiver, Instant::now() + Duration::from_secs(1)).is_err());
}
