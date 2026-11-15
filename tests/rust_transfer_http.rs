//! Actual loopback HTTP streams against the production capability router.
#[path = "local_support/mod.rs"]
mod local_support;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use telegram_mcp::{state::State, transfer::Transfers};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    task::JoinHandle,
};

struct Server {
    _directory: tempfile::TempDir,
    state: Arc<State>,
    transfers: Arc<Transfers>,
    address: std::net::SocketAddr,
    task: JoinHandle<()>,
}
impl Server {
    async fn start() -> Self {
        let (directory, state) = local_support::fixture();
        let transfers = Arc::new(Transfers::new(state.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = telegram_mcp::transfer::http::router(transfers.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _directory: directory,
            state,
            transfers,
            address,
            task,
        }
    }
    fn base(&self) -> String {
        format!("http://{}", self.address)
    }
    fn prepare(&self, name: &str, cap: u64) -> Value {
        self.transfers.prepare(name, &self.base(), cap).unwrap()
    }
    fn uploads(&self) -> PathBuf {
        self.state.file_roots()[0].join("uploads")
    }
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }
    async fn raw(&self, path: &str, body: &[u8], extra_headers: &str) -> String {
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        stream
            .write_all(
                format!(
                    "PUT {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{extra_headers}\r\n",
                    self.address
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(body).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn path(value: &Value) -> &std::path::Path {
    std::path::Path::new(value["file_path"].as_str().unwrap())
}
fn route(value: &Value) -> String {
    url::Url::parse(value["url"].as_str().unwrap())
        .unwrap()
        .path()
        .to_owned()
}

#[tokio::test]
async fn one_use_upload_writes_exact_bytes_and_replay_cannot_replace_them() {
    let server = Server::start().await;
    let prepared = server.prepare("سلام 🙂.bin", 32);
    let client = server.client();
    let url = prepared["url"].as_str().unwrap();
    let response = client
        .put(url)
        .body(vec![0, 128, 255, 10])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["size"], 4);
    assert_eq!(std::fs::read(path(&prepared)).unwrap(), [0, 128, 255, 10]);
    assert_eq!(
        client
            .put(url)
            .body("replacement")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(path(&prepared)).unwrap(), [0, 128, 255, 10]);
    assert_eq!(std::fs::read_dir(server.uploads()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path(&prepared))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn concurrent_replays_allow_exactly_one_request_to_claim_the_ticket() {
    let server = Server::start().await;
    let prepared = server.prepare("concurrent.bin", 32);
    let url = prepared["url"].as_str().unwrap();
    let client = server.client();
    let (first, second) = tokio::join!(
        client.put(url).body("first").send(),
        client.put(url).body("second").send()
    );
    let mut statuses = [
        first.unwrap().status().as_u16(),
        second.unwrap().status().as_u16(),
    ];
    statuses.sort_unstable();
    assert_eq!(statuses, [201, 404]);
    let bytes = std::fs::read(path(&prepared)).unwrap();
    assert!(bytes == b"first" || bytes == b"second");
}

#[tokio::test]
async fn competing_tickets_for_one_filename_never_clobber_the_first_upload() {
    let server = Server::start().await;
    let a = server.prepare("same.bin", 32);
    let b = server.prepare("same.bin", 32);
    let client = server.client();
    assert_eq!(
        client
            .put(a["url"].as_str().unwrap())
            .body("original")
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    assert_eq!(
        client
            .put(b["url"].as_str().unwrap())
            .body("replacement")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(std::fs::read(path(&a)).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(server.uploads()).unwrap().count(), 1);
}

#[tokio::test]
async fn declared_oversize_is_rejected_and_consumes_only_its_upload_capability() {
    let server = Server::start().await;
    let prepared = server.prepare("too-large.bin", 3);
    let client = server.client();
    let url = prepared["url"].as_str().unwrap();
    assert_eq!(
        client.put(url).body("four").send().await.unwrap().status(),
        413
    );
    assert!(!path(&prepared).exists());
    assert_eq!(std::fs::read_dir(server.uploads()).unwrap().count(), 0);
    assert_eq!(
        client.put(url).body("ok").send().await.unwrap().status(),
        404
    );
}

#[tokio::test]
async fn chunked_upload_enforces_actual_stream_size_and_removes_partial_files() {
    let server = Server::start().await;
    let prepared = server.prepare("chunked.bin", 3);
    let response = server
        .raw(
            &route(&prepared),
            b"2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n",
            "Transfer-Encoding: chunked\r\n",
        )
        .await;
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(!path(&prepared).exists());
    assert_eq!(std::fs::read_dir(server.uploads()).unwrap().count(), 0);
}

#[tokio::test]
async fn interrupted_upload_removes_staging_file_and_never_publishes() {
    let server = Server::start().await;
    let prepared = server.prepare("interrupted.bin", 32);
    let _response = server
        .raw(&route(&prepared), b"partial", "Content-Length: 20\r\n")
        .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(server.uploads())
                .unwrap()
                .next()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!path(&prepared).exists());
}

#[tokio::test]
async fn upload_and_download_capabilities_are_not_interchangeable() {
    let server = Server::start().await;
    let prepared = server.prepare("scoped.bin", 32);
    let token = route(&prepared).rsplit('/').next().unwrap().to_owned();
    let client = server.client();
    assert_eq!(
        client
            .get(format!("{}/media/{token}", server.base()))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .put(prepared["url"].as_str().unwrap())
            .body("ok")
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    let link = server
        .transfers
        .download_link(path(&prepared), &server.base())
        .unwrap();
    let token = route(&link).rsplit('/').next().unwrap().to_owned();
    assert_eq!(
        client
            .put(format!("{}/upload/{token}", server.base()))
            .body("bad")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(link["url"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"ok"
    );
}

#[tokio::test]
async fn download_streams_exact_bytes_with_safe_attachment_and_no_cache_headers() {
    let server = Server::start().await;
    let file = server.state.file_roots()[0].join("سلام 🙂.bin");
    std::fs::write(&file, [0, 128, 255]).unwrap();
    let link = server
        .transfers
        .download_link(&file, &server.base())
        .unwrap();
    let response = server
        .client()
        .get(link["url"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-length"], "3");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("%F0%9F%99%82")
    );
    assert_eq!(response.bytes().await.unwrap().as_ref(), &[0, 128, 255]);
}

#[tokio::test]
async fn changed_download_source_invalidates_the_old_capability() {
    let server = Server::start().await;
    let file = server.state.file_roots()[0].join("changed.bin");
    std::fs::write(&file, b"old").unwrap();
    let link = server
        .transfers
        .download_link(&file, &server.base())
        .unwrap();
    std::fs::write(&file, b"different bytes").unwrap();
    assert_eq!(
        server
            .client()
            .get(link["url"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
}

#[tokio::test]
async fn replacement_file_cannot_reuse_a_download_ticket_by_restoring_mtime_and_size() {
    let server = Server::start().await;
    let file = server.state.file_roots()[0].join("replace.bin");
    std::fs::write(&file, b"old").unwrap();
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    let link = server
        .transfers
        .download_link(&file, &server.base())
        .unwrap();
    std::fs::rename(&file, file.with_extension("saved")).unwrap();
    std::fs::write(&file, b"new").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        server
            .client()
            .get(link["url"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
}

#[tokio::test]
async fn random_tokens_and_path_traversal_never_read_or_create_files() {
    let server = Server::start().await;
    let client = server.client();
    for suffix in [
        "/media/unknown",
        "/upload/unknown",
        "/media/%2e%2e%2fprivate",
        "/upload/%2e%2e%2ffile",
    ] {
        let response = if suffix.starts_with("/upload") {
            client
                .put(format!("{}{suffix}", server.base()))
                .body("bad")
                .send()
                .await
                .unwrap()
        } else {
            client
                .get(format!("{}{suffix}", server.base()))
                .send()
                .await
                .unwrap()
        };
        assert!(matches!(response.status().as_u16(), 400 | 404));
    }
    assert_eq!(
        std::fs::read_dir(&server.state.file_roots()[0])
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn upload_parent_swapped_after_issuing_the_ticket_cannot_escape_roots() {
    let server = Server::start().await;
    let prepared = server.prepare("escape.bin", 32);
    let outside = server._directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::rename(server.uploads(), server.uploads().with_extension("saved")).unwrap();
    std::os::unix::fs::symlink(&outside, server.uploads()).unwrap();
    assert_eq!(
        server
            .client()
            .put(prepared["url"].as_str().unwrap())
            .body("no")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_mid_stream_directory_swap_cleans_the_held_directory_without_publication() {
    let server = Server::start().await;
    let prepared = server.prepare("race.bin", 32);
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream.write_all(format!("PUT {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n", route(&prepared), server.address).as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(server.uploads())
                .unwrap()
                .next()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved = server.uploads().with_extension("saved");
    std::fs::rename(server.uploads(), &saved).unwrap();
    let outside = server._directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, server.uploads()).unwrap();
    stream.write_all(b"3\r\ndef\r\n0\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 403") || response.starts_with("HTTP/1.1 409"),
        "{response}"
    );
    assert_eq!(
        std::fs::read_dir(&saved).unwrap().count(),
        0,
        "A failed upload leaked staging bytes in the renamed directory"
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn download_symlink_replacement_is_rechecked_before_opening() {
    let server = Server::start().await;
    let file = server.state.file_roots()[0].join("link.bin");
    std::fs::write(&file, b"old").unwrap();
    let link = server
        .transfers
        .download_link(&file, &server.base())
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    let outside = server._directory.path().join("outside.bin");
    std::fs::write(&outside, b"private").unwrap();
    std::os::unix::fs::symlink(&outside, &file).unwrap();
    assert_eq!(
        server
            .client()
            .get(link["url"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"private");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_holds_the_upload_parent_against_rename_until_publication() {
    let server = Server::start().await;
    let prepared = server.prepare("held.bin", 32);
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream.write_all(format!("PUT {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n",route(&prepared),server.address).as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(server.uploads())
                .unwrap()
                .next()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(std::fs::rename(server.uploads(), server.uploads().with_extension("renamed")).is_err());
    stream.write_all(b"3\r\ndef\r\n0\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 201"), "{response}");
    assert_eq!(std::fs::read(path(&prepared)).unwrap(), b"abcdef");
    assert_eq!(std::fs::read_dir(server.uploads()).unwrap().count(), 1);
}
