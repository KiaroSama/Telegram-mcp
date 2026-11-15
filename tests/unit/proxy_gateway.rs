use super::*;
use serde_json::json;

fn state() -> (tempfile::TempDir, Arc<State>) {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let state = Arc::new(State::at(dir.path().join("private"), vec![allowed]).unwrap());
    (dir, state)
}
#[test]
fn session_datacenter_mapping_prefers_exact_endpoint_and_rejects_invalid_ids() {
    let (_dir, state) = state();
    let target: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    state
        .write_json(
            "work.session.json",
            &json!({"dc_options":[{"id":4,"ipv4":target.to_string(),"ipv6":"[::1]:443"}]}),
        )
        .unwrap();
    assert_eq!(datacenter(&state, "work", target).unwrap(), 4);
    assert_eq!(
        datacenter(&state, "work", "[::1]:443".parse().unwrap()).unwrap(),
        4
    );
    for value in [
        json!({"dc_options":[{"id":"invalid","ipv4":target.to_string()}]}),
        json!({"dc_options":[{"id":999999,"ipv4":target.to_string()}]}),
    ] {
        state.write_json("work.session.json", &value).unwrap();
        assert!(datacenter(&state, "work", target).is_err());
    }
    assert!(datacenter(&state, "work", "127.0.0.1:12346".parse().unwrap()).is_err());
    let default = grammers_session::SessionData::default();
    let option = default
        .dc_options
        .values()
        .find(|entry| entry.id > 0)
        .unwrap();
    assert_eq!(
        datacenter(&state, "work", SocketAddr::V4(option.ipv4)).unwrap(),
        option.id as i16
    );
}
fn res_pq(long: bool) -> Vec<u8> {
    let mut bytes = vec![0; 36];
    if long {
        bytes.extend([254, 1, 0, 0, 9, 0, 0, 0]);
    } else {
        bytes.extend([1, 9, 0, 0]);
    }
    bytes.extend(0x1cb5c415u32.to_le_bytes());
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(12345i64.to_le_bytes());
    bytes
}
#[test]
fn res_pq_parser_accepts_both_tl_byte_lengths_and_refuses_malformed_vectors() {
    validate_res_pq(&res_pq(false)).unwrap();
    validate_res_pq(&res_pq(true)).unwrap();
    for body in [
        vec![],
        vec![0; 4097],
        {
            let mut b = res_pq(false);
            b[36] = 255;
            b
        },
        {
            let mut b = res_pq(true);
            b[37..40].copy_from_slice(&[1, 2, 0]);
            b
        },
        {
            let mut b = res_pq(false);
            b.truncate(45);
            b
        },
        {
            let mut b = res_pq(false);
            b[40] = 0;
            b
        },
        {
            let mut b = res_pq(false);
            b[44..48].copy_from_slice(&0i32.to_le_bytes());
            b
        },
        {
            let mut b = res_pq(false);
            b[44..48].copy_from_slice(&65i32.to_le_bytes());
            b
        },
    ] {
        assert!(validate_res_pq(&body).is_err());
    }
}
#[tokio::test]
async fn failed_route_backoff_is_immediate_and_does_not_attempt_a_socket() {
    let (_dir, state) = state();
    let context = ContextState {
        state,
        label: "work".into(),
        snapshot: ConfigSnapshot {
            values: Default::default(),
            digest: "fixture".into(),
            path: "fixture.env".into(),
            process_keys: Default::default(),
        },
        status: Arc::new(Mutex::new(Value::Null)),
        failed_until: Mutex::new(Some(Instant::now() + Duration::from_secs(30))),
    };
    let error = select(&context, "127.0.0.1:1".parse().unwrap())
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("backoff"));
    assert_eq!(*context.status.lock().unwrap(), Value::Null);
}
async fn auth_exchange(bytes: Vec<u8>) -> (Result<SocketAddr>, Vec<u8>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        authenticate(&mut socket, "owner", "password").await
    });
    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(&bytes).await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = vec![];
    let _ = client.read_to_end(&mut response).await;
    (server.await.unwrap(), response)
}
fn credentials() -> Vec<u8> {
    let mut bytes = vec![5, 1, 2, 1, 5];
    bytes.extend(b"owner");
    bytes.push(8);
    bytes.extend(b"password");
    bytes
}
#[tokio::test]
async fn private_gateway_refuses_unauthenticated_and_malformed_connect_requests() {
    let (result, response) = auth_exchange(vec![5, 1, 0]).await;
    assert!(result.is_err());
    assert_eq!(response, [5, 255]);
    let mut invalid = credentials();
    invalid[4] = 1;
    invalid.drain(6..10);
    let (result, response) = auth_exchange(invalid).await;
    assert!(result.is_err());
    assert_eq!(&response[..2], [5, 2]);
    for request in [
        vec![5, 2, 0, 1],
        vec![5, 1, 1, 1],
        vec![5, 1, 0, 3],
        vec![5, 1, 0, 1, 127, 0, 0, 1, 0, 0],
    ] {
        let mut bytes = credentials();
        bytes.extend(request);
        let (result, response) = auth_exchange(bytes).await;
        assert!(result.is_err());
        assert_eq!(&response[..4], [5, 2, 1, 0]);
    }
}
#[tokio::test]
async fn private_gateway_accepts_literal_ipv6_without_dns_resolution() {
    let mut bytes = credentials();
    bytes.extend([5, 1, 0, 4]);
    bytes.extend(std::net::Ipv6Addr::LOCALHOST.octets());
    bytes.extend(443u16.to_be_bytes());
    let (result, response) = auth_exchange(bytes).await;
    assert_eq!(result.unwrap(), "[::1]:443".parse().unwrap());
    assert_eq!(response, [5, 2, 1, 0]);
}
#[tokio::test]
async fn exhausting_every_route_sets_backoff_without_publishing_a_successful_route() {
    let (_dir, state) = state();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap();
    drop(listener);
    let context = ContextState {
        state,
        label: "work".into(),
        snapshot: ConfigSnapshot {
            values: Default::default(),
            digest: "fixture".into(),
            path: "fixture.env".into(),
            process_keys: Default::default(),
        },
        status: Arc::new(Mutex::new(Value::Null)),
        failed_until: Mutex::new(None),
    };
    assert!(select(&context, target).await.is_err());
    assert!(context.failed_until.lock().unwrap().is_some());
    assert_eq!(*context.status.lock().unwrap(), Value::Null);
    assert!(
        select(&context, target)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("backoff")
    );
}
