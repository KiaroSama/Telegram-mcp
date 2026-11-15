use super::support::*;
use serde_json::json;
use std::{net::Ipv4Addr, sync::Arc};
use telegram_mcp::proxy::{self, mtproxy};
use tokio::{io::AsyncWriteExt, net::TcpListener};
#[tokio::test]
async fn failed_configured_proxy_falls_back_before_owner_bytes_are_sent() {
    let (_temp, state) = state();
    let (target, _telegram, seen) = telegram(false).await;
    let (address, _proxy, requests) = http_proxy(target, true).await;
    let url = format!("http://{address}");
    let route = proxy::route(state, &snapshot(&[("TELEGRAM_PROXY_URL", &url)]), "default")
        .await
        .unwrap();
    let mut socket = connect_gateway(&route, target).await.unwrap();
    let owner = b"private!";
    socket
        .write_all(&mtproxy::full_frame(owner, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(mtproxy::read_full(&mut socket, 0).await.unwrap(), owner);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|row| row.as_slice() == owner)
            .count(),
        1
    );
    assert_eq!(route.metadata()["route"], "direct");
}
#[tokio::test]
async fn new_pool_entries_are_used_by_the_existing_gateway() {
    let (_temp, state) = state();
    let (route_target, _telegram, _) = telegram(false).await;
    let (address, _proxy, requests) = http_proxy(route_target, false).await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let blocked = listener.local_addr().unwrap();
    drop(listener);
    let route = proxy::route(Arc::clone(&state), &snapshot(&[]), "work")
        .await
        .unwrap();
    let p = proxy("http", address);
    let id = p.fingerprint().unwrap();
    state
        .update_json("proxies-v1.json", json!({}), |store| {
            store[&id] = json!({"proxy":p,"pool":"work","healthy":true,"latency_ms":1});
            Ok(())
        })
        .unwrap();
    let mut socket = connect_gateway(&route, blocked).await.unwrap();
    socket
        .write_all(&mtproxy::full_frame(b"payload!", 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        mtproxy::read_full(&mut socket, 0).await.unwrap(),
        b"payload!"
    );
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert_eq!(route.metadata()["route"], id);
}
#[test]
fn corrupt_legacy_pool_is_preserved_and_not_silently_ignored() {
    let (_temp, state) = state();
    state
        .write_json(
            "proxy-pool.json",
            &json!({"shared":"invalid","accounts":{},"routes":{}}),
        )
        .unwrap();
    let path = state.root().join("proxy-pool.json");
    let before = std::fs::read(&path).unwrap();
    assert!(proxy::candidates(&state, &snapshot(&[]), "default").is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(!state.root().join("proxy-legacy-import-v1.json").exists());
}
#[test]
fn explicit_native_pool_record_wins_over_legacy_projection() {
    let (_temp, state) = state();
    let p = proxy("http", "127.0.0.1:8080".parse().unwrap());
    let id = p.fingerprint().unwrap();
    state
        .write_json(
            "proxy-pool.json",
            &json!({"shared":[p],"accounts":{},"routes":{}}),
        )
        .unwrap();
    state
        .write_json(
            "proxies-v1.json",
            &json!({&id:{"proxy":p,"pool":"work","healthy":false,"last_result":"native_fixture"}}),
        )
        .unwrap();
    let rows = proxy::candidates(&state, &snapshot(&[]), "work").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        state.read_json("proxies-v1.json", json!({})).unwrap()[&id]["last_result"],
        "native_fixture"
    );
}
