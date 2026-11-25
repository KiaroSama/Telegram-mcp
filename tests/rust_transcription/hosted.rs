use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
type Requests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
struct Server {
    url: String,
    requests: Requests,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(status: u16, body: Value, location: Option<String>) -> Server {
    server_on(std::net::Ipv4Addr::LOCALHOST.into(), status, body, location).await
}
async fn server_on(
    ip: std::net::IpAddr,
    status: u16,
    body: Value,
    location: Option<String>,
) -> Server {
    let listener = TcpListener::bind((ip, 0)).await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let recorded = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut headers = vec![];
            while !headers.ends_with(b"\r\n\r\n") {
                let Ok(byte) = socket.read_u8().await else {
                    return;
                };
                headers.push(byte);
                if headers.len() > 16384 {
                    return;
                }
            }
            let header = String::from_utf8(headers).unwrap();
            let len = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            // Record the method before reading any body, including redirected GETs.
            let index = {
                let mut requests = recorded.lock().unwrap();
                let index = requests.len();
                requests.push((header.clone(), vec![]));
                index
            };
            if len > 1_048_576 {
                return;
            }
            let mut data = vec![0; len];
            if socket.read_exact(&mut data).await.is_err() {
                return;
            }
            recorded.lock().unwrap()[index].1 = data;
            let text = body.to_string();
            let redirect = location
                .as_ref()
                .map(|url| format!("Location: {url}\r\n"))
                .unwrap_or_default();
            let response = format!(
                "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{redirect}\r\n{text}",
                text.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    Server {
        url,
        requests,
        task,
    }
}

async fn proxy_probe_child(mode: &str) -> bool {
    if std::env::var("TELEGRAM_TEST_ASR_PROXY_MODE").as_deref() != Ok(mode) {
        return false;
    }
    let endpoint = std::env::var("TELEGRAM_TEST_ASR_PROXY_ENDPOINT").unwrap();
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let mut values = vec![("TELEGRAM_TRANSCRIBE_OPENAI_URL", endpoint.as_str())];
    if mode == "https" {
        values.push(("TELEGRAM_TRANSCRIBE_OPENAI_API_KEY", "synthetic-asr-key"));
    }
    let result = transcribe::transcribe_with(&api, &settings(&values, &request), &request).await;
    if mode == "http" {
        assert_eq!(result.unwrap()["text"], "local endpoint");
    } else {
        assert!(result.is_err(), "The synthetic HTTPS proxy refuses CONNECT");
    }
    true
}

async fn run_proxy_probe(name: &str, mode: &str, endpoint: &str, proxy: &str) {
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("TELEGRAM_")
            || matches!(name.as_ref(), "OPENAI_API_KEY" | "GROQ_API_KEY")
        {
            child.env_remove(key);
        }
    }
    child
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("TELEGRAM_TEST_ASR_PROXY_MODE", mode)
        .env("TELEGRAM_TEST_ASR_PROXY_ENDPOINT", endpoint)
        .env("HTTP_PROXY", proxy)
        .env("http_proxy", proxy)
        .env("HTTPS_PROXY", proxy)
        .env("https_proxy", proxy)
        .env("ALL_PROXY", "")
        .env("all_proxy", "")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env_remove("REQUEST_METHOD")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), child.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated ASR proxy probe failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(4096)]),
        String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
    );
}

#[path = "../support/nonloopback.rs"]
mod interface;
use interface::nonloopback_address;

#[tokio::test]
async fn loopback_hosted_transcription_bypasses_ambient_remote_proxy() {
    if proxy_probe_child("http").await {
        return;
    }
    let endpoint = server(200, json!({"text":"local endpoint"}), None).await;
    let proxy = server_on(
        nonloopback_address(),
        200,
        json!({"text":"wrong proxy"}),
        None,
    )
    .await;
    run_proxy_probe(
        "hosted_tests::loopback_hosted_transcription_bypasses_ambient_remote_proxy",
        "http",
        &endpoint.url,
        proxy.url.trim_end_matches("/v1"),
    )
    .await;
    assert!(
        proxy.requests.lock().unwrap().is_empty(),
        "A loopback-only recording reached an ambient nonloopback proxy"
    );
    let requests = endpoint.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("POST /v1/audio/transcriptions "));
    assert!(
        !requests[0]
            .0
            .to_ascii_lowercase()
            .contains("authorization:")
    );
    let audio = b"OggS synthetic voice fixture, not a real recording";
    assert!(
        requests[0]
            .1
            .windows(audio.len())
            .any(|bytes| bytes == audio)
    );
    assert!(
        requests[0]
            .1
            .windows(13)
            .any(|bytes| bytes == b"name=\"model\"\r")
    );
}

#[tokio::test]
async fn https_hosted_transcription_retains_proxy_routing_without_exposing_audio_or_key() {
    if proxy_probe_child("https").await {
        return;
    }
    let proxy = server_on(nonloopback_address(), 502, json!({}), None).await;
    run_proxy_probe("hosted_tests::https_hosted_transcription_retains_proxy_routing_without_exposing_audio_or_key", "https", "https://asr.example.invalid/v1", proxy.url.trim_end_matches("/v1")).await;
    let requests = proxy.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .0
            .starts_with("CONNECT asr.example.invalid:443 HTTP/1.1\r\n")
    );
    assert!(requests[0].1.is_empty());
    assert!(
        !requests[0]
            .0
            .to_ascii_lowercase()
            .contains("authorization:")
    );
    assert!(!requests[0].0.contains("synthetic-asr-key"));
}
#[tokio::test]
async fn hosted_multipart_preserves_audio_model_language_and_utf8_text() {
    let server = server(200, json!({"text":"سلام 東京","language":"fa"}), None).await;
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let config = settings(
        &[
            ("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url),
            ("TELEGRAM_TRANSCRIBE_OPENAI_MODEL", "fixture-model"),
            ("TELEGRAM_TRANSCRIBE_LANGUAGE", "fa"),
        ],
        &request,
    );
    let result = transcribe::transcribe_with(&api, &config, &request)
        .await
        .unwrap();
    assert_eq!(result["text"], "سلام 東京");
    assert_eq!(result["audio_uploaded"], true);
    assert_eq!(api.downloads.load(Ordering::Relaxed), 1);
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].0.starts_with("POST /v1/audio/transcriptions "));
    assert!(
        !requests[0]
            .0
            .to_ascii_lowercase()
            .contains("authorization:")
    );
    let body = &requests[0].1;
    for expected in [
        b"name=\"model\"".as_slice(),
        b"fixture-model",
        b"name=\"language\"",
        b"filename=\"voice.ogg\"",
        b"OggS synthetic voice fixture, not a real recording",
    ] {
        assert!(body.windows(expected.len()).any(|bytes| bytes == expected));
    }
}
#[tokio::test]
async fn endpoint_failures_never_echo_keys_or_provider_body() {
    let server = server(401, json!({"error":"provider-private-detail"}), None).await;
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let config = settings(&[("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url)], &request);
    let error = transcribe::transcribe_with(&api, &config, &request)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("401"));
    assert!(!error.contains("provider-private-detail"));
    assert!(!error.contains("synthetic-private-key"));
}
#[tokio::test]
async fn uploaded_recording_is_never_forwarded_through_redirects() {
    let target = server(200, json!({"text":"wrong destination"}), None).await;
    let redirect = server(302, json!({}), Some(target.url.clone())).await;
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let config = settings(
        &[("TELEGRAM_TRANSCRIBE_OPENAI_URL", &redirect.url)],
        &request,
    );
    assert!(
        transcribe::transcribe_with(&api, &config, &request)
            .await
            .is_err()
    );
    assert_eq!(redirect.requests.lock().unwrap().len(), 1);
    assert!(target.requests.lock().unwrap().is_empty());
    assert!(
        !target.task.is_finished(),
        "Target recorder must remain alive rather than hide a task panic"
    );
}
#[tokio::test]
async fn distinct_hosted_models_cannot_share_cached_text() {
    let server = server(200, json!({"text":"model output"}), None).await;
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let first = settings(
        &[
            ("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url),
            ("TELEGRAM_TRANSCRIBE_OPENAI_MODEL", "model-a"),
        ],
        &request,
    );
    let second = settings(
        &[
            ("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url),
            ("TELEGRAM_TRANSCRIBE_OPENAI_MODEL", "model-b"),
        ],
        &request,
    );
    transcribe::transcribe_with(&api, &first, &request)
        .await
        .unwrap();
    assert_eq!(
        transcribe::transcribe_with(&api, &second, &request)
            .await
            .unwrap()["cached"],
        false
    );
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn recorder_observes_bodyless_redirect_gets() {
    let server = server(200, json!({}), None).await;
    reqwest::get(&server.url).await.unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("GET "));
    assert!(requests[0].1.is_empty());
    assert!(!server.task.is_finished());
}
