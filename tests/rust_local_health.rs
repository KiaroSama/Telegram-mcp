mod local_support;
use local_support::fixture;
use serde_json::{Value, json};
use std::io::{Read, Write};
use telegram_mcp::account_manager::{config_digest, session_health_with_state};

fn service(response: Value) -> (String, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    service_on(listener, response)
}

fn service_on(
    listener: std::net::TcpListener,
    response: Value,
) -> (String, std::thread::JoinHandle<String>) {
    let address = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") && request.len() < 16_384 {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let body = serde_json::to_vec(&response).unwrap();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
        stream.write_all(&body).unwrap();
        String::from_utf8(request).unwrap()
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
            &json!({"url":url,"token":"t".repeat(32),"pid":1}),
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
    assert!(
        request
            .to_lowercase()
            .contains(&format!("authorization: bearer {}", "t".repeat(32)))
    );
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
            &json!({"url":url,"token":"t".repeat(32)}),
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
    state
        .write_json(
            "http-service.json",
            &json!({"url":"http://example.com/mcp","token":"t".repeat(32)}),
        )
        .unwrap();
    assert!(
        session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
            .await
            .is_err()
    );
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
async fn ipv6_loopback_health_keeps_bearer_auth_and_exact_generation_checks() {
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
            &json!({"url":url,"token":"t".repeat(32)}),
        )
        .unwrap();
    let result = session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
        .await
        .unwrap();
    assert_eq!(result["status"], "healthy");
    let request = thread.join().unwrap();
    assert!(request.starts_with("GET /health?"));
    assert!(
        request
            .to_lowercase()
            .contains(&format!("authorization: bearer {}", "t".repeat(32)))
    );
    state
        .write_json(
            "http-service.json",
            &json!({"url":"http://[2001:db8::1]/mcp","token":"t".repeat(32)}),
        )
        .unwrap();
    assert!(
        session_health_with_state(&state, &env, "TELEGRAM_SESSION_NAME_WORK")
            .await
            .is_err()
    );
}
