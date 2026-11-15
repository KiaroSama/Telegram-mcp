mod local_dispatch_support;
#[path = "rust_proxy_transport/support.rs"]
mod transport_support;
use local_dispatch_support::{Api, call, isolated};
use serde_json::json;
use telegram_mcp::{state::State, tools::local::proxies::parse_proxy};

#[tokio::test]
async fn proxy_pools_persist_protocol_results_and_remove_only_selected_memberships() {
    if !isolated("proxy_pools_persist_protocol_results_and_remove_only_selected_memberships") {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    let (target, _telegram, _) = transport_support::telegram(false).await;
    let (address, _proxy, requests) = transport_support::http_proxy(target, false).await;
    let link = format!("http://synthetic-owner:synthetic-password@{address}");
    let added = call(
        &api,
        "add_proxies",
        json!({"text":format!("{link} {link} ftp://private:credential@invalid.example") }),
    )
    .await
    .unwrap();
    assert_eq!(added["added"].as_array().unwrap().len(), 1);
    assert_eq!(added["duplicates"], 1);
    assert_eq!(added["invalid"].as_array().unwrap().len(), 1);
    assert_eq!(added["added"][0]["result"], "telegram_transport_reachable");
    assert!(!added.to_string().contains("synthetic-password"));
    assert!(!added.to_string().contains("credential"));
    let id = added["added"][0]["id"].as_str().unwrap().to_owned();
    assert!(requests.lock().unwrap()[0].starts_with("CONNECT 149.154.167.50:443 HTTP/1.1"));
    let duplicate = call(&api, "add_proxies", json!({"pool":"WORK","text":link}))
        .await
        .unwrap();
    assert_eq!(duplicate["duplicates"], 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
    let listed = call(&api, "list_proxies", json!({"pool":"work"}))
        .await
        .unwrap();
    assert_eq!(listed["proxies"][0]["healthy"], true);
    assert_eq!(listed["proxies"][0]["pools"], json!(["shared", "work"]));
    assert!(!listed.to_string().contains("synthetic-owner"));
    let tested = call(&api, "test_proxies", json!({"pool":"work","ids":[id]}))
        .await
        .unwrap();
    assert_eq!(
        tested["results"][0]["result"],
        "telegram_transport_reachable"
    );
    assert_eq!(
        call(&api, "remove_proxies", json!({"pool":"work","ids":[id]}))
            .await
            .unwrap()["removed"],
        1
    );
    assert_eq!(
        call(&api, "list_proxies", json!({"pool":"work"}))
            .await
            .unwrap()["proxies"],
        json!([])
    );
    assert_eq!(
        call(&api, "list_proxies", json!({"pool":"shared"}))
            .await
            .unwrap()["proxies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    state
        .update_json("proxies-v1.json", json!({}), |store| {
            store[&id]["healthy"] = json!(false);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        call(&api, "remove_proxies", json!({"unhealthy":true}))
            .await
            .unwrap()["removed"],
        1
    );
    assert_eq!(
        call(&api, "list_proxies", json!({})).await.unwrap()["proxies"],
        json!([])
    );
    assert!(call(&api, "remove_proxies", json!({})).await.is_err());
    assert!(
        call(&api, "test_proxies", json!({"ids":"wrong"}))
            .await
            .is_err()
    );
    assert!(
        call(&api, "remove_proxies", json!({"ids":[7]}))
            .await
            .is_err()
    );
    assert!(call(&api, "add_proxies", json!({})).await.is_err());
    assert_eq!(
        call(&api, "get_connection_route", json!({"account":"work"}))
            .await
            .unwrap()["verification"],
        "not_probed"
    );
    assert_eq!(api.calls()[0].0, "local.connection_route");
}

#[tokio::test]
async fn source_proxy_import_reads_nested_link_fields_and_bounds_transport_probes() {
    if !isolated("source_proxy_import_reads_nested_link_fields_and_bounds_transport_probes") {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    let (target, _telegram, _) = transport_support::telegram(false).await;
    let (address, _proxy, requests) = transport_support::http_proxy(target, false).await;
    let links = (0..17)
        .map(|index| format!("http://synthetic{index}:private@{address}"))
        .collect::<Vec<_>>();
    api.reply("messages.getHistory",json!({"messages":[{"message":links[..8].join(" "),"entities":[{"url":links[8..16].join(" ")}]},{"text":links[16]}]}));
    let result = call(
        &api,
        "add_proxies",
        json!({"source":"channel","pool":"work"}),
    )
    .await
    .unwrap();
    let rows = result["added"].as_array().unwrap();
    assert_eq!(rows.len(), 17);
    assert_eq!(
        rows.iter()
            .filter(|row| row["result"] == "telegram_transport_reachable")
            .count(),
        16
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["result"] == "not_tested")
            .count(),
        1
    );
    assert_eq!(requests.lock().unwrap().len(), 16);
    let pending = rows
        .iter()
        .find(|row| row["result"] == "not_tested")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    assert!(state.read_json("proxies-v1.json", json!({})).unwrap()[pending]["healthy"].is_null());
    assert_eq!(api.calls()[0].0, "messages.getHistory");
    assert_eq!(api.calls()[0].1["limit"], 200);
    assert_eq!(
        call(&api, "test_proxies", json!({"ids":["missing"]}))
            .await
            .unwrap()["results"],
        json!([])
    );
}

#[tokio::test]
async fn failed_proxy_probe_is_not_healthy_and_corrupt_pool_is_preserved() {
    if !isolated("failed_proxy_probe_is_not_healthy_and_corrupt_pool_is_preserved") {
        return;
    }
    let api = Api::default();
    let state = State::global().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let result = call(
        &api,
        "add_proxies",
        json!({"text":format!("http://{address}"),"pool":"work"}),
    )
    .await
    .unwrap();
    assert_eq!(result["added"][0]["result"], "connection_failed");
    assert_eq!(
        call(&api, "list_proxies", json!({})).await.unwrap()["proxies"][0]["healthy"],
        false
    );
    state
        .write_json("proxies-v1.json", &json!(["invalid pool"]))
        .unwrap();
    let before = std::fs::read(state.root().join("proxies-v1.json")).unwrap();
    assert!(call(&api, "list_proxies", json!({})).await.is_err());
    assert!(
        call(
            &api,
            "add_proxies",
            json!({"text":format!("http://{address}")})
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(state.root().join("proxies-v1.json")).unwrap(),
        before
    );
    let proxy = parse_proxy(&format!("http://{address}")).unwrap();
    let id = proxy.fingerprint().unwrap();
    state
        .write_json("proxies-v1.json", &json!({id:"corrupt existing record"}))
        .unwrap();
    let before = std::fs::read(state.root().join("proxies-v1.json")).unwrap();
    assert!(
        call(
            &api,
            "add_proxies",
            json!({"text":format!("http://{address}")})
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(state.root().join("proxies-v1.json")).unwrap(),
        before
    );
    assert!(api.calls().is_empty());
}
