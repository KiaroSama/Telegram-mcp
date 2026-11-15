//! Real loopback wire-protocol tests; no Telegram account or external service.
#[path = "rust_proxy_transport/support.rs"]
mod support;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::*;
use telegram_mcp::proxy::{
    self,
    mtproxy::{self, Obfuscation, Secret},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[tokio::test]
async fn http_connect_carries_real_mtproto_probe() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, requests) = http_proxy(target, false).await;
    let p = proxy("http", address);
    proxy::probe(&p, target, 2).await.unwrap();
    assert!(requests.lock().unwrap()[0].starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")));
}
#[tokio::test]
async fn http_proxy_auth_is_encoded_in_the_connect_header() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, requests) = http_proxy(target, false).await;
    let mut p = proxy("http", address);
    p.username = Some("owner".into());
    p.password = Some("secret".into());
    proxy::probe(&p, target, 2).await.unwrap();
    assert!(
        requests.lock().unwrap()[0].contains("Proxy-Authorization: Basic b3duZXI6c2VjcmV0\r\n")
    );
}
#[tokio::test]
async fn rejected_http_connect_is_not_reported_reachable() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, _) = http_proxy(target, true).await;
    assert!(
        proxy::probe(&proxy("http", address), target, 2)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn socks4_routes_the_actual_telegram_transport() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, seen) = socks_proxy(target, 4, None, None).await;
    let mut p = proxy("socks4", address);
    p.username = Some("owner".into());
    proxy::probe(&p, target, 2).await.unwrap();
    assert_eq!(seen.lock().unwrap()[0]["username"], "owner");
}
#[tokio::test]
async fn socks5_routes_with_username_and_password() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, seen) =
        socks_proxy(target, 5, Some("owner".into()), Some("secret".into())).await;
    let mut p = proxy("socks5", address);
    p.username = Some("owner".into());
    p.password = Some("secret".into());
    proxy::probe(&p, target, 2).await.unwrap();
    assert_eq!(seen.lock().unwrap()[0]["password"], "secret");
}
#[tokio::test]
async fn wrong_socks5_password_is_refused() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, _) =
        socks_proxy(target, 5, Some("owner".into()), Some("correct".into())).await;
    let mut p = proxy("socks5", address);
    p.username = Some("owner".into());
    p.password = Some("wrong".into());
    assert!(proxy::probe(&p, target, 2).await.is_err());
}
#[tokio::test]
async fn a_proxy_tcp_socket_without_res_pq_is_not_reachable() {
    let (target, _telegram, _) = telegram(true).await;
    let (address, _proxy, _) = http_proxy(target, false).await;
    assert!(
        proxy::probe(&proxy("http", address), target, 2)
            .await
            .unwrap_err()
            .to_string()
            .contains("nonce")
    );
}
#[tokio::test]
async fn mtproxy_standard_secret_completes_real_obfuscated_probe() {
    let secret = synthetic_secret(0x11);
    let (address, _server) = mtproxy_server(&secret).await;
    let mut p = proxy("mtproto", address);
    p.secret = Some(secret);
    proxy::probe(&p, "149.154.167.50:443".parse().unwrap(), 2)
        .await
        .unwrap();
}
#[tokio::test]
async fn mtproxy_dd_secret_completes_real_padded_probe() {
    let secret = format!("dd{}", synthetic_secret(0x11));
    let (address, _server) = mtproxy_server(&secret).await;
    let mut p = proxy("mtproto", address);
    p.secret = Some(secret);
    proxy::probe(&p, "149.154.167.50:443".parse().unwrap(), 2)
        .await
        .unwrap();
}
#[tokio::test]
async fn dropped_route_closes_the_private_gateway() {
    let (_temp, state) = state();
    let route = proxy::route(state, &snapshot(&[]), "default")
        .await
        .unwrap();
    let url = url::Url::parse(route.proxy_url.as_ref().unwrap()).unwrap();
    let address = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    drop(route);
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(address))
            .await
            .unwrap()
            .is_err()
    );
}
#[tokio::test]
async fn private_gateway_refuses_unauthenticated_clients() {
    let (_temp, state) = state();
    let route = proxy::route(state, &snapshot(&[]), "default")
        .await
        .unwrap();
    let url = url::Url::parse(route.proxy_url.as_ref().unwrap()).unwrap();
    let mut socket = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap()))
        .await
        .unwrap();
    socket.write_all(&[5, 1, 0]).await.unwrap();
    let mut response = [0; 2];
    socket.read_exact(&mut response).await.unwrap();
    assert_eq!(response, [5, 255]);
}
#[tokio::test]
async fn gateway_probes_anonymously_then_forwards_owner_payload_once() {
    let (_temp, state) = state();
    let (target, _telegram, seen) = telegram(false).await;
    let route = proxy::route(state, &snapshot(&[]), "default")
        .await
        .unwrap();
    let mut socket = connect_gateway(&route, target).await.unwrap();
    let owner = b"owner payload!!!";
    socket
        .write_all(&mtproxy::full_frame(owner, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(mtproxy::read_full(&mut socket, 0).await.unwrap(), owner);
    let rows = seen.lock().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().filter(|row| row.as_slice() == owner).count(), 1);
    assert_eq!(route.metadata()["route"], "direct");
}
#[tokio::test]
async fn configured_http_route_is_used_by_the_sender_gateway() {
    let (_temp, state) = state();
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, requests) = http_proxy(target, false).await;
    let url = format!("http://{address}");
    let route = proxy::route(
        Arc::clone(&state),
        &snapshot(&[("TELEGRAM_PROXY_URL", &url)]),
        "default",
    )
    .await
    .unwrap();
    let mut socket = connect_gateway(&route, target).await.unwrap();
    socket
        .write_all(&mtproxy::full_frame(b"payload!", 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        mtproxy::read_full(&mut socket, 0).await.unwrap(),
        b"payload!"
    );
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert_eq!(route.metadata()["route"], "env");
    assert_eq!(route.metadata()["kind"], "http");
}
#[test]
fn account_proxy_settings_override_global_settings() {
    let (_temp, state) = state();
    let config = snapshot(&[
        ("TELEGRAM_PROXY_TYPE", "http"),
        ("TELEGRAM_PROXY_HOST", "global.example"),
        ("TELEGRAM_PROXY_PORT", "80"),
        ("TELEGRAM_PROXY_HOST_WORK", "account.example"),
        ("TELEGRAM_PROXY_PORT_WORK", "8080"),
    ]);
    let choices = proxy::candidates(&state, &config, "work").unwrap();
    assert_eq!(choices[0].proxy.as_ref().unwrap().host, "account.example");
    assert_eq!(choices[0].proxy.as_ref().unwrap().port, 8080);
    assert_eq!(choices[1].id, "direct");
}
#[test]
fn partial_proxy_configuration_is_refused_instead_of_ignored() {
    let (_temp, state) = state();
    assert!(
        proxy::candidates(
            &state,
            &snapshot(&[("TELEGRAM_PROXY_HOST", "proxy.example")]),
            "default"
        )
        .is_err()
    );
}
#[test]
fn unsupported_proxy_type_is_refused_without_direct_fallback() {
    let (_temp, state) = state();
    assert!(
        proxy::candidates(
            &state,
            &snapshot(&[("TELEGRAM_PROXY_TYPE", "socks9")]),
            "default"
        )
        .is_err()
    );
}
#[test]
fn own_pool_precedes_shared_and_health_is_sorted() {
    let (_temp, state) = state();
    for (id, pool, healthy, latency) in [
        ("shared", "shared", Some(true), 1),
        ("slow", "work", Some(true), 90),
        ("fast", "work", Some(true), 10),
        ("untested", "work", None, 0),
        ("failed", "work", Some(false), 1),
    ] {
        state.update_json("proxies-v1.json",json!({}),|store|{store[id]=json!({"proxy":proxy("http","127.0.0.1:8080".parse().unwrap()),"pool":pool,"healthy":healthy,"latency_ms":latency});Ok(())}).unwrap();
    }
    let choices = proxy::candidates(&state, &snapshot(&[]), "work").unwrap();
    assert_eq!(
        choices.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["direct", "fast", "slow", "untested", "failed"]
    );
}
#[test]
fn legacy_proxy_migration_preserves_bytes_and_duplicate_memberships() {
    let (_temp, state) = state();
    let p = proxy("http", "127.0.0.1:8080".parse().unwrap());
    let mut entry = serde_json::to_value(&p).unwrap();
    entry["healthy"] = json!(true);
    state
        .write_json(
            "proxy-pool.json",
            &json!({"shared":[entry.clone()],"accounts":{"work":[entry]},"routes":{}}),
        )
        .unwrap();
    let path = state.root().join("proxy-pool.json");
    let before = std::fs::read(&path).unwrap();
    let choices = proxy::candidates(&state, &snapshot(&[]), "work").unwrap();
    assert_eq!(choices.len(), 2);
    let native = state.read_json("proxies-v1.json", json!({})).unwrap();
    let record = &native[&p.fingerprint().unwrap()];
    assert!(proxy::record_in_pool(record, "shared"));
    assert!(proxy::record_in_pool(record, "work"));
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[test]
fn route_descriptions_never_contain_credentials() {
    let mut p = proxy("http", "127.0.0.1:8080".parse().unwrap());
    p.username = Some("private-user".into());
    p.password = Some("private-password".into());
    let text = proxy::Candidate {
        id: "env".into(),
        proxy: Some(p),
    }
    .describe()
    .to_string();
    assert!(!text.contains("private-user"));
    assert!(!text.contains("private-password"));
}
#[test]
fn obfuscation_streams_round_trip_across_different_chunk_sizes() {
    let secret = Secret::parse(&synthetic_secret(0x11)).unwrap();
    let mut client = Obfuscation::new(&secret, 4).unwrap();
    let (mut server, dc) = Obfuscation::from_client_header(client.header, &secret).unwrap();
    assert_eq!(dc, 4);
    let mut payload = b"a continuous encrypted transport payload".to_vec();
    client.encrypt(&mut payload[..3]);
    client.encrypt(&mut payload[3..]);
    server.decrypt(&mut payload);
    assert_eq!(payload, b"a continuous encrypted transport payload");
    let mut reply = b"reply".to_vec();
    server.encrypt(&mut reply);
    client.decrypt(&mut reply[..2]);
    client.decrypt(&mut reply[2..]);
    assert_eq!(reply, b"reply");
}
#[test]
fn wrong_secret_cannot_decode_the_mtproxy_header() {
    let secret = Secret::parse(&synthetic_secret(0x11)).unwrap();
    let wrong = Secret::parse(&synthetic_secret(0x22)).unwrap();
    let client = Obfuscation::new(&secret, 2).unwrap();
    assert!(Obfuscation::from_client_header(client.header, &wrong).is_err());
}
#[test]
fn extreme_datacenter_id_returns_an_error_instead_of_panicking() {
    let secret = Secret::parse(&synthetic_secret(0x11)).unwrap();
    assert!(Obfuscation::new(&secret, i16::MIN).is_err());
}
#[tokio::test]
async fn full_transport_rejects_a_tampered_checksum() {
    let (mut writer, mut reader) = tokio::io::duplex(128);
    let mut frame = mtproxy::full_frame(b"payload!", 0).unwrap();
    frame[10] ^= 1;
    writer.write_all(&frame).await.unwrap();
    assert!(
        mtproxy::read_full(&mut reader, 0)
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
}
#[tokio::test]
async fn full_transport_checks_sequence_before_delivering_payload() {
    let (mut writer, mut reader) = tokio::io::duplex(128);
    writer
        .write_all(&mtproxy::full_frame(b"payload!", 1).unwrap())
        .await
        .unwrap();
    assert!(
        mtproxy::read_full(&mut reader, 0)
            .await
            .unwrap_err()
            .to_string()
            .contains("sequence")
    );
}
#[tokio::test]
async fn oversized_full_frame_is_refused_before_allocating_its_body() {
    let (mut writer, mut reader) = tokio::io::duplex(128);
    writer
        .write_all(&((mtproxy::MAX_FRAME + 4) as i32).to_le_bytes())
        .await
        .unwrap();
    assert!(mtproxy::read_full(&mut reader, 0).await.is_err());
}
#[path = "rust_proxy_transport/failover.rs"]
mod failover_tests;
#[path = "rust_proxy_transport/faketls.rs"]
mod faketls_tests;

#[tokio::test]
async fn socks5_utf8_credentials_round_trip_without_replacement() {
    let (target, _telegram, _) = telegram(false).await;
    let (address, _proxy, seen) =
        socks_proxy(target, 5, Some("حساب".into()), Some("東京-pass".into())).await;
    let mut p = proxy("socks5", address);
    p.username = Some("حساب".into());
    p.password = Some("東京-pass".into());
    proxy::probe(&p, target, 2).await.unwrap();
    assert_eq!(seen.lock().unwrap()[0]["username"], "حساب");
    assert_eq!(seen.lock().unwrap()[0]["password"], "東京-pass");
}

#[test]
fn all_sixteen_padding_lengths_are_removed_from_unauthenticated_envelopes() {
    let mut request = vec![0; 40];
    request[20..24].copy_from_slice(&0xbe7e8ef1u32.to_le_bytes());
    let original = reply(&request, false).unwrap();
    for padding in 0..16 {
        let mut packet = original.clone();
        packet.extend(vec![0x73; padding]);
        mtproxy::strip_padding(&mut packet).unwrap();
        assert_eq!(packet, original);
    }
}
#[test]
fn all_sixteen_padding_lengths_are_removed_from_encrypted_envelopes() {
    let original = vec![1u8; 24 + 64];
    for padding in 0..16 {
        let mut packet = original.clone();
        packet.extend(vec![0x73; padding]);
        mtproxy::strip_padding(&mut packet).unwrap();
        assert_eq!(packet, original);
    }
}
#[test]
fn excessive_unauthenticated_padding_is_refused() {
    let mut request = vec![0; 40];
    request[20..24].copy_from_slice(&0xbe7e8ef1u32.to_le_bytes());
    let mut packet = reply(&request, false).unwrap();
    packet.extend([0; 16]);
    assert!(mtproxy::strip_padding(&mut packet).is_err());
}
#[test]
fn padded_transport_errors_retain_their_error_code_only() {
    let mut packet = (-404i32).to_le_bytes().to_vec();
    packet.extend([8; 15]);
    mtproxy::strip_padding(&mut packet).unwrap();
    assert_eq!(packet, (-404i32).to_le_bytes());
}
#[test]
fn truncated_encrypted_envelope_is_refused() {
    let mut packet = vec![1; 39];
    assert!(mtproxy::strip_padding(&mut packet).is_err());
}
