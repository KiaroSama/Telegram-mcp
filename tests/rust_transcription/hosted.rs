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
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
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
                .unwrap();
            if len > 1_048_576 {
                return;
            }
            let mut data = vec![0; len];
            if socket.read_exact(&mut data).await.is_err() {
                return;
            }
            recorded.lock().unwrap().push((header, data));
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
#[tokio::test]
async fn hosted_multipart_preserves_audio_model_language_and_utf8_text() {
    let server = server(200, json!({"text":"سلام 東京","language":"fa"}), None).await;
    let api = Api::new("unused");
    let request = json!({"chat_id":7,"message_id":10,"engine":"openai"});
    let config = settings(
        &[
            ("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url),
            (
                "TELEGRAM_TRANSCRIBE_OPENAI_API_KEY",
                "synthetic-transcription-key",
            ),
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
        requests[0]
            .0
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-transcription-key")
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
    let config = settings(
        &[
            ("TELEGRAM_TRANSCRIBE_OPENAI_URL", &server.url),
            (
                "TELEGRAM_TRANSCRIBE_OPENAI_API_KEY",
                "synthetic-private-key",
            ),
        ],
        &request,
    );
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
