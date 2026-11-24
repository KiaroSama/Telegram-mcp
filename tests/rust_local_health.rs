mod local_support;
use hmac::{Hmac, Mac};
use local_support::fixture;
use serde_json::{Value, json};
use sha2::Sha256;
use std::io::{Read, Write};
use telegram_mcp::account_manager::{config_digest, session_health_with_state};

fn service(response: Value) -> (String, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    service_on(listener, response)
}

#[derive(Clone, Copy)]
enum ResponseMode {
    Signed,
    Unsigned,
    WrongBody,
    WrongNonce,
    WrongTarget,
    WrongStatus,
    Chunked,
}

// Independent implementation of the documented v2 wire format, so a shared
// signing/verification implementation error cannot make this fixture pass.
fn proof(purpose: &str, nonce: &str, target: &str, status: u16, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice("t".repeat(32).as_bytes()).unwrap();
    let status_bytes = status.to_be_bytes();
    for bytes in [
        b"telegram-mcp-health-v2\0".as_slice(),
        purpose.as_bytes(),
        b"\0",
        nonce.as_bytes(),
        b"\0",
        target.as_bytes(),
        b"\0",
        &status_bytes,
        body,
    ] {
        mac.update(bytes);
    }
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.trim()))
}

fn service_on(
    listener: std::net::TcpListener,
    response: Value,
) -> (String, std::thread::JoinHandle<String>) {
    service_mode(listener, response, ResponseMode::Signed)
}

fn service_mode(
    listener: std::net::TcpListener,
    response: Value,
    mode: ResponseMode,
) -> (String, std::thread::JoinHandle<String>) {
    let address = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(error) => panic!("No health request reached the controlled listener: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") && request.len() < 16_384 {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(header(&request, "authorization").is_none());
        assert!(!request.contains(&"t".repeat(32)));
        let nonce =
            header(&request, "x-telegram-health-nonce").expect("Health request must have a nonce");
        assert!(nonce.len() == 64 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let target = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        assert_eq!(
            header(&request, "x-telegram-health-proof"),
            Some(proof("request", nonce, target, 0, &[]).as_str())
        );
        let body = serde_json::to_vec(&response).unwrap();
        let altered_nonce = format!(
            "{}{}",
            if nonce.starts_with('0') { '1' } else { '0' },
            &nonce[1..]
        );
        let signature = proof(
            "response",
            if matches!(mode, ResponseMode::WrongNonce) {
                &altered_nonce
            } else {
                nonce
            },
            if matches!(mode, ResponseMode::WrongTarget) {
                "/unrelated-health"
            } else {
                target
            },
            if matches!(mode, ResponseMode::WrongStatus) {
                500
            } else {
                200
            },
            if matches!(mode, ResponseMode::WrongBody) {
                b"different response"
            } else {
                &body
            },
        );
        let auth = if matches!(mode, ResponseMode::Unsigned) {
            String::new()
        } else {
            format!("x-telegram-health-proof: {signature}\r\n")
        };
        if matches!(mode, ResponseMode::Chunked) {
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{auth}Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            let _ = write!(stream, "{:x}\r\n", body.len());
            let _ = stream.write_all(&body);
            let _ = stream.write_all(b"\r\n0\r\n\r\n");
        } else {
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            let _ = stream.write_all(&body);
        }
        request
    });
    (format!("http://{address}/mcp"), thread)
}

#[tokio::test]
async fn health_uses_existing_authenticated_loopback_owner_and_exact_source_digest() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state
        .write_json("work.session.json", &json!({"private_fixture":true}))
        .unwrap();
    let digest = config_digest("work");
    let (url, thread) = service(
        json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":digest,"user_id":"42"}),
    );
    state
        .write_json(
            "http-service.json",
            &json!({"url":url,"token":"t".repeat(32),"pid":1,"health_protocol":2}),
        )
        .unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "healthy");
    assert_eq!(result["repairable"], false);
    assert_eq!(result["user_id"], "42");
    let request = thread.join().unwrap();
    assert!(request.starts_with("GET /health?"));
    assert!(header(&request, "authorization").is_none());
    assert!(header(&request, "x-telegram-health-proof").is_some());
    assert!(request.contains(&digest));
    assert!(!request.contains("private_fixture"));
}

#[tokio::test]
async fn health_from_a_different_configuration_generation_is_unknown() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let (url, thread) = service(
        json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":"different"}),
    );
    state
        .write_json(
            "http-service.json",
            &json!({"url":url,"token":"t".repeat(32),"health_protocol":2}),
        )
        .unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["repairable"], false);
    thread.join().unwrap();
}

#[tokio::test]
async fn service_absence_is_unknown_instead_of_invalid_authorization() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["repairable"], false);
}

#[tokio::test]
async fn health_never_sends_private_service_token_to_a_remote_host() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    for host in [
        "example.com",
        "localhost.example",
        "127.0.0.1.example",
        "localhost.",
    ] {
        state.write_json("http-service.json", &json!({"url":format!("http://{host}/mcp"),"token":"t".repeat(32),"health_protocol":2})).unwrap();
        assert!(
            session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn exact_localhost_health_keeps_its_hostname_while_using_the_loopback_listener() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let (url, thread) = service(
        json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work")}),
    );
    state.write_json("http-service.json",&json!({"url":url.replace("127.0.0.1","localhost"),"token":"t".repeat(32),"health_protocol":2})).unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "healthy");
    let request = thread.join().unwrap();
    assert!(header(&request, "host").unwrap().starts_with("localhost:"));
}

#[tokio::test]
async fn malformed_import_session_is_refused_without_opening_a_runtime_connection() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_STRING_WORK=bad\n").unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_STRING_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "invalid");
    assert_eq!(result["repairable"], true);
}

#[tokio::test]
async fn ipv6_loopback_health_keeps_authenticated_proofs_and_exact_generation_checks() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let digest = config_digest("work");
    let listener = std::net::TcpListener::bind("[::1]:0")
        .expect("Native health service requires operating-system loopback IPv6 support");
    let (url, thread) = service_on(
        listener,
        json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":digest}),
    );
    state
        .write_json(
            "http-service.json",
            &json!({"url":url,"token":"t".repeat(32),"health_protocol":2}),
        )
        .unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "healthy");
    let request = thread.join().unwrap();
    assert!(request.starts_with("GET /health?"));
    assert!(header(&request, "authorization").is_none());
    assert!(header(&request, "x-telegram-health-proof").is_some());
    state
        .write_json(
            "http-service.json",
            &json!({"url":"http://[2001:db8::1]/mcp","token":"t".repeat(32),"health_protocol":2}),
        )
        .unwrap();
    assert!(
        session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn forged_or_unsigned_loopback_health_never_supplies_an_identity_or_repair_permission() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let mut nonces = std::collections::BTreeSet::new();
    for mode in [
        ResponseMode::Unsigned,
        ResponseMode::WrongBody,
        ResponseMode::WrongNonce,
        ResponseMode::WrongTarget,
        ResponseMode::WrongStatus,
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (url, thread) = service_mode(
            listener,
            json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work"),"user_id":"99"}),
            mode,
        );
        state
            .write_json(
                "http-service.json",
                &json!({"url":url,"token":"t".repeat(32),"health_protocol":2}),
            )
            .unwrap();
        let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
            .await
            .unwrap();
        assert_eq!(result["status"], "busy");
        assert_eq!(result["repairable"], false);
        assert!(result.get("user_id").is_none());
        let request = thread.join().unwrap();
        assert!(
            nonces.insert(
                header(&request, "x-telegram-health-nonce")
                    .unwrap()
                    .to_owned()
            ),
            "Each request needs a fresh replay-resistant nonce"
        );
    }
}

#[tokio::test]
async fn oversized_chunked_health_is_refused_even_with_a_valid_response_signature() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (url, thread) = service_mode(
        listener,
        json!({"status":"healthy","config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work"),"user_id":"42","padding":"p".repeat(65_537)}),
        ResponseMode::Chunked,
    );
    state
        .write_json(
            "http-service.json",
            &json!({"url":url,"token":"t".repeat(32),"health_protocol":2}),
        )
        .unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "busy");
    assert_eq!(result["repairable"], false);
    assert!(result.get("user_id").is_none());
    thread.join().unwrap();
}

#[tokio::test]
async fn legacy_discovery_never_sends_a_reusable_bearer_to_a_rebound_port() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state.write_json("work.session.json", &json!({})).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    state.write_json("http-service.json",&json!({"url":format!("http://{}/mcp",listener.local_addr().unwrap()),"token":"t".repeat(32)})).unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["repairable"], false);
    assert!(matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
}
