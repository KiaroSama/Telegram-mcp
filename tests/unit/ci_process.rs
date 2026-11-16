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

#[test]
fn native_release_headers_reject_wrong_architectures_and_truncated_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-image");
    let targets = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ];
    for target in targets {
        let mut bytes = vec![0u8; 512];
        let arm = target.starts_with("aarch64");
        if target.contains("linux") {
            bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&(if arm { 183u16 } else { 62 }).to_le_bytes());
        } else if target.contains("darwin") {
            bytes[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
            bytes[4..8]
                .copy_from_slice(&(if arm { 0x0100000cu32 } else { 0x01000007 }).to_le_bytes());
            bytes[12..16].copy_from_slice(&2u32.to_le_bytes());
        } else {
            bytes[..2].copy_from_slice(b"MZ");
            bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
            bytes[64..68].copy_from_slice(b"PE\0\0");
            bytes[68..70].copy_from_slice(&(if arm { 0xaa64u16 } else { 0x8664 }).to_le_bytes());
            bytes[84..86].copy_from_slice(&240u16.to_le_bytes());
            bytes[86..88].copy_from_slice(&2u16.to_le_bytes());
            bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        for expected in targets {
            assert_eq!(
                verify_binary_target(&path, expected).is_ok(),
                target == expected,
                "{target} image labeled {expected}"
            );
        }
        for length in [0, 2, 16, 63] {
            std::fs::write(&path, &bytes[..length]).unwrap();
            assert!(verify_binary_target(&path, target).is_err());
        }
        if target.contains("windows") {
            for (size, flags, length) in [(0u16, 2u16, 512), (240, 0x2002, 512), (240, 2, 100)] {
                let mut invalid = bytes.clone();
                invalid[84..86].copy_from_slice(&size.to_le_bytes());
                invalid[86..88].copy_from_slice(&flags.to_le_bytes());
                std::fs::write(&path, &invalid[..length]).unwrap();
                assert!(verify_binary_target(&path, target).is_err());
            }
        }
    }
    let native_target = if cfg!(target_os = "linux") {
        if cfg!(target_arch = "aarch64") {
            targets[1]
        } else {
            targets[0]
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            targets[3]
        } else {
            targets[2]
        }
    } else if cfg!(target_arch = "aarch64") {
        targets[5]
    } else {
        targets[4]
    };
    verify_binary_target(&std::env::current_exe().unwrap(), native_target).unwrap();
    assert!(verify_binary_target(&path, "x86_64-unknown-freebsd").is_err());
}
