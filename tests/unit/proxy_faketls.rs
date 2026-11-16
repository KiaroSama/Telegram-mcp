use super::*;
use tokio::{net::TcpListener, task::JoinHandle};
async fn bridge() -> (tokio::io::DuplexStream, TcpStream, JoinHandle<Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connection = TcpStream::connect(listener.local_addr().unwrap());
    let (client, server) = tokio::join!(connection, listener.accept());
    let (local, bridge) = tokio::io::duplex(65536);
    let task = tokio::spawn(records(bridge, client.unwrap()));
    (local, server.unwrap().0, task)
}
fn record(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind, 3, 3];
    out.extend((payload.len() as u16).to_be_bytes());
    out.extend(payload);
    out
}
#[tokio::test]
async fn native_tls_record_bridge_forwards_actual_data_and_graceful_shutdown() {
    let (mut local, mut network, task) = bridge().await;
    local.write_all(b"outgoing bytes").await.unwrap();
    local.flush().await.unwrap();
    let mut ccs = [0; 6];
    network.read_exact(&mut ccs).await.unwrap();
    assert_eq!(ccs, [0x14, 3, 3, 0, 1, 1]);
    let mut bytes = vec![0; 19];
    network.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, record(0x17, b"outgoing bytes"));
    local.write_all(b"next").await.unwrap();
    let mut next = [0; 9];
    network.read_exact(&mut next).await.unwrap();
    assert_eq!(next.as_slice(), record(0x17, b"next"));
    network.write_all(&record(0x17, b"reply")).await.unwrap();
    let mut response = [0; 5];
    local.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"reply");
    local.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn control_and_empty_records_are_ignored_and_data_resets_the_consecutive_limit() {
    let (mut local, mut network, task) = bridge().await;
    let mut records = vec![];
    for _ in 0..128 {
        records.extend(record(0x14, &[1]));
    }
    records.extend(record(0x17, b"A"));
    for _ in 0..128 {
        records.extend(record(0x17, &[]));
    }
    records.extend(record(0x17, b"B"));
    network.write_all(&records).await.unwrap();
    let mut response = [0; 2];
    local.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"AB");
    drop(local);
    let _ = task.await.unwrap();
}
#[tokio::test]
async fn malformed_tls_records_are_rejected_before_their_bytes_reach_the_local_stream() {
    for (bytes, reason) in [
        (vec![0x16, 3, 3, 0, 1, 1], "non-data"),
        (vec![0x17, 3, 1, 0, 1, 1], "non-data"),
        (vec![0x17, 3, 3, 0x40, 0x19], "byte bound"),
        (record(0x14, &[2]), "change-cipher-spec"),
        (
            (0..129).flat_map(|_| record(0x14, &[1])).collect(),
            "control records",
        ),
        (
            (0..129).flat_map(|_| record(0x17, &[])).collect(),
            "empty records",
        ),
    ] {
        let (mut local, mut network, task) = bridge().await;
        network.write_all(&bytes).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(error.contains(reason), "{error}");
        assert!(local.read_u8().await.is_err());
    }
}
