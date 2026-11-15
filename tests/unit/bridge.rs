use super::*;

fn record(endpoint: &str) -> Value {
    json!({"url":endpoint,"pid":std::process::id(),"token":"synthetic-private-discovery-0123456789"})
}

#[test]
fn bridge_descriptor_accepts_only_verified_loopback_mcp_urls() {
    for endpoint in [
        "http://127.0.0.1:8765/mcp",
        "http://[::1]:8765/mcp",
        "http://localhost:80/mcp",
    ] {
        assert!(descriptor(&record(endpoint), None).is_ok(), "{endpoint}");
    }
    for endpoint in [
        "https://127.0.0.1:8765/mcp",
        "http://192.0.2.1:8765/mcp",
        "http://localhost/mcp",
        "http://user@127.0.0.1:8765/mcp",
        "http://127.0.0.1:8765/mcp?token=private",
        "http://127.0.0.1:8765/mcp#fragment",
        "http://127.0.0.1:8765/other",
    ] {
        assert!(descriptor(&record(endpoint), None).is_err(), "{endpoint}");
    }
}

#[test]
fn bridge_descriptor_checks_requested_endpoint_before_sending_credentials() {
    let record = record("http://127.0.0.1:8765/mcp");
    assert!(descriptor(&record, Some("http://localhost:8765/mcp")).is_ok());
    assert!(descriptor(&record, Some("http://127.0.0.1:8766/mcp")).is_err());
    assert!(descriptor(&record, Some("http://192.0.2.1:8765/mcp")).is_err());
}

#[test]
fn bridge_descriptor_requires_private_authentication_and_nonreusable_process_evidence() {
    for field in ["pid", "token"] {
        let mut value = record("http://127.0.0.1:8765/mcp");
        value.as_object_mut().unwrap().remove(field);
        assert!(descriptor(&value, None).is_err());
    }
    let mut value = record("http://127.0.0.1:8765/mcp");
    for token in ["short", "line\nbreak-and-private-credential-0123456789"] {
        value["token"] = token.into();
        assert!(descriptor(&value, None).is_err());
    }
    value["token"] = "synthetic-private-discovery-0123456789".into();
    for pid in [json!(0), json!(-1), json!(u64::MAX), json!("123")] {
        value["pid"] = pid;
        assert!(descriptor(&value, None).is_err());
    }
}

#[test]
fn bridge_process_identity_is_stable_and_rejects_another_image() {
    let actual = std::env::current_exe().unwrap().canonicalize().unwrap();
    let first = ownership::verified(std::process::id(), &actual).unwrap();
    assert_eq!(
        first,
        ownership::verified(std::process::id(), &actual).unwrap()
    );
    let temp = tempfile::NamedTempFile::new().unwrap();
    assert!(ownership::verified(std::process::id(), temp.path()).is_err());
    assert!(ownership::identity(u32::MAX).is_err());
}

#[cfg(windows)]
#[test]
fn bridge_private_pipe_handles_keep_io_and_lose_only_inheritance() {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    let mut file = tempfile::tempfile().unwrap();
    let handle = file.as_raw_handle();
    // This handle is local to the test and has no live child; no process-wide
    // standard stream flags or parallel test state are modified.
    assert_ne!(
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
        0
    );
    let mut before = 0;
    assert_ne!(unsafe { GetHandleInformation(handle, &mut before) }, 0);
    assert_ne!(before & HANDLE_FLAG_INHERIT, 0);
    startup::prevent_inheritance(handle).unwrap();
    let mut after = 0;
    assert_ne!(unsafe { GetHandleInformation(handle, &mut after) }, 0);
    assert_eq!(after, before & !HANDLE_FLAG_INHERIT);
    file.write_all(b"client-pipe-io-survives").unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut actual = String::new();
    file.read_to_string(&mut actual).unwrap();
    assert_eq!(actual, "client-pipe-io-survives");
    let error = startup::prevent_inheritance(INVALID_HANDLE_VALUE).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot isolate the shared service from client pipes"
    );
}
