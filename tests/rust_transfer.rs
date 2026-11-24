//! Capability/file boundaries exercised without Telegram credentials.
#[path = "local_support/mod.rs"]
mod local_support;
use serde_json::json;
use std::path::Path;
use telegram_mcp::transfer::{Transfers, validate_base, validate_filename};

#[tokio::test]
async fn download_body_ownership_bounds_open_streams_and_returns_permits() {
    use axum::{
        extract::{Path as AxumPath, State},
        http::StatusCode,
    };
    use std::sync::Arc;
    let (_directory, state) = local_support::fixture();
    let file = state.file_roots()[0].join("bounded.bin");
    std::fs::write(&file, b"owned stream bytes").unwrap();
    let transfers = Arc::new(Transfers::new(state));
    let link = transfers
        .download_link(&file, "http://127.0.0.1:1")
        .unwrap();
    let token = url::Url::parse(link["url"].as_str().unwrap())
        .unwrap()
        .path()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let mut held = Vec::new();
    let mut rejected = false;
    for _ in 0..65 {
        let response = telegram_mcp::transfer::http::download(
            State(transfers.clone()),
            AxumPath(token.clone()),
        )
        .await;
        if response.status() == StatusCode::SERVICE_UNAVAILABLE {
            rejected = true;
            break;
        }
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert!(
        rejected,
        "One reusable capability must not open unbounded download files"
    );
    assert!(!held.is_empty());
    drop(held.pop());
    let response =
        telegram_mcp::transfer::http::download(State(transfers.clone()), AxumPath(token.clone()))
            .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"owned stream bytes");
    let response = telegram_mcp::transfer::http::download(State(transfers), AxumPath(token)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "EOF must release the stream permit"
    );
}

#[test]
fn filenames_reject_path_traversal_and_portable_device_names() {
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "a:b",
        " trailing ",
        "trailing.",
        "line\nfeed",
        "NUL.txt",
        "con",
        "COM1.txt",
        "LPT9",
        "COM¹.txt",
        "COM²",
        "COM³.wav",
        "LPT¹",
        "LPT².txt",
        "LPT³",
        "CONIN$",
        "CONOUT$.txt",
    ] {
        assert!(validate_filename(name).is_err(), "accepted {name:?}");
    }
    for name in ["سلام 🙂.ogg", "report.final.zip", "COM10.txt", "console.txt"] {
        validate_filename(name).unwrap();
    }
}

#[test]
fn public_urls_require_tls_except_for_literal_loopback_hosts() {
    for base in [
        "http://example.org",
        "file:///tmp",
        "https://user:password@example.org",
        "https://example.org/?secret=x",
        "https://example.org/#fragment",
        "http://localhost.evil.test",
        "http://127.0.0.1.evil.test",
    ] {
        assert!(validate_base(base).is_err(), "accepted {base}");
    }
    assert_eq!(
        validate_base("https://example.org/files").unwrap(),
        "https://example.org/files"
    );
    validate_base("http://127.0.0.1:8080").unwrap();
    validate_base("http://[::1]:8080/").unwrap();
    validate_base("http://localhost:8080/").unwrap();
}

#[test]
fn upload_budget_is_checked_before_issuing_a_capability() {
    let (_dir, state) = local_support::fixture();
    let transfers = Transfers::new(state);
    for budget in [0, 4 * 1024 * 1024 * 1024 + 1] {
        assert!(
            transfers
                .prepare("recording.ogg", "http://localhost", budget)
                .is_err()
        );
    }
    let value = transfers
        .prepare("recording.ogg", "http://localhost", 3)
        .unwrap();
    assert_eq!(value["max_bytes"], 3);
    assert_eq!(value["method"], "PUT");
    assert_eq!(value["single_use"], true);
    assert!(!Path::new(value["file_path"].as_str().unwrap()).exists());
}

#[test]
fn preparing_an_existing_filename_never_changes_the_original() {
    let (_dir, state) = local_support::fixture();
    let transfers = Transfers::new(state);
    let prepared = transfers
        .prepare("existing.bin", "http://localhost", 32)
        .unwrap();
    let path = Path::new(prepared["file_path"].as_str().unwrap());
    std::fs::write(path, b"owner's existing bytes").unwrap();
    assert!(
        transfers
            .prepare("existing.bin", "http://localhost", 32)
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"owner's existing bytes");
}

#[test]
fn outstanding_capabilities_are_bounded_without_touching_destinations() {
    let (_dir, state) = local_support::fixture();
    let transfers = Transfers::new(state);
    for i in 0..64 {
        transfers
            .prepare(&format!("file-{i}.bin"), "https://example.test", 16)
            .unwrap();
    }
    assert!(
        transfers
            .prepare("overflow.bin", "https://example.test", 16)
            .is_err()
    );
}

#[test]
fn download_capabilities_refuse_private_state_repository_and_parent_paths() {
    let (dir, state) = local_support::fixture();
    let transfers = Transfers::new(state.clone());
    state
        .write_json("authorization.json", &json!({"secret":"fixture"}))
        .unwrap();
    std::fs::write(dir.path().join("installation/.env"), b"private fixture").unwrap();
    for path in [
        state.root().join("authorization.json"),
        dir.path().join("installation/.env"),
        state.file_roots()[0].join("../private/authorization.json"),
        state.file_roots()[0].clone(),
    ] {
        assert!(
            transfers.download_link(&path, "http://localhost").is_err(),
            "accepted {}",
            path.display()
        );
    }
}

#[test]
fn missing_file_roots_cannot_create_upload_destinations() {
    let (dir, _state) = local_support::fixture();
    let state = std::sync::Arc::new(
        telegram_mcp::state::State::at_with_installation(
            dir.path().join("other-private"),
            vec![],
            dir.path().join("installation"),
        )
        .unwrap(),
    );
    assert!(
        Transfers::new(state)
            .prepare("x.bin", "http://localhost", 3)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn upload_directory_and_files_remain_private_on_unix() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, state) = local_support::fixture();
    let uploads = state.file_roots()[0].join("uploads");
    std::fs::create_dir(&uploads).unwrap();
    std::fs::set_permissions(&uploads, std::fs::Permissions::from_mode(0o777)).unwrap();
    Transfers::new(state)
        .prepare("x.bin", "http://localhost", 3)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&uploads).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn prepare_refuses_upload_directory_symlinks() {
    let (dir, state) = local_support::fixture();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, state.file_roots()[0].join("uploads")).unwrap();
    assert!(
        Transfers::new(state)
            .prepare("x.bin", "http://localhost", 3)
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn stale_http_discovery_never_publishes_a_url_for_another_process() {
    let (_dir, state) = local_support::fixture();
    for record in [
        json!({"url":"http://127.0.0.1:12345/mcp"}),
        json!({"url":"http://127.0.0.1:12345/mcp","pid":u64::from(std::process::id())+1}),
        json!({"url":"http://127.0.0.1:12345/mcp","pid":-1}),
    ] {
        state.write_json("http-service.json", &record).unwrap();
        assert!(telegram_mcp::transfer::base_url(&state).is_err());
    }
}

#[test]
fn best_effort_download_attachment_preserves_completed_saves_on_link_failure() {
    let (_dir, state) = local_support::fixture();
    let transfers = Transfers::new(state.clone());
    let file = state.file_roots()[0].join("saved.bin");
    std::fs::write(&file, b"completed").unwrap();
    let mut result = json!({"path":file,"message_id":7,"download":{"size":9}});
    transfers.attach_download(&mut result, "http://localhost:8080");
    assert!(
        result["download_url"]
            .as_str()
            .unwrap()
            .starts_with("http://localhost:8080/media/")
    );
    assert_eq!(result["download_url_is_temporary_credential"], true);
    let first = result.clone();
    transfers.attach_download(&mut result, "http://localhost:8080");
    assert_eq!(result, first);
    let mut private = json!({"path":state.root().join("authorization.json"),"saved":true});
    let original = private.clone();
    transfers.attach_download(&mut private, "http://localhost:8080");
    assert_eq!(private, original);
    assert_eq!(std::fs::read(file).unwrap(), b"completed");
}
