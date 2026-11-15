use super::*;
use tokio::net::TcpListener;

fn proxy(kind: &str, address: SocketAddr) -> Proxy {
    Proxy {
        kind: kind.into(),
        host: address.ip().to_string(),
        port: address.port(),
        secret: None,
        username: None,
        password: None,
    }
}
async fn socks_reply(method: [u8; 2], reply: Vec<u8>, target: SocketAddr) -> Result<Stream> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut hello = [0; 3];
        socket.read_exact(&mut hello).await.unwrap();
        socket.write_all(&method).await.unwrap();
        if method != [5, 0] {
            return;
        }
        let mut request = vec![0; if target.is_ipv4() { 10 } else { 22 }];
        socket.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[..3], [5, 1, 0]);
        socket.write_all(&reply).await.unwrap();
    });
    let result = connect(Some(&proxy("socks5", address)), target, 2).await;
    task.await.unwrap();
    result
}
#[tokio::test]
async fn socks5_connect_accepts_domain_and_ipv6_bound_addresses() {
    for (reply, target) in [
        (
            vec![5, 0, 0, 3, 3, b'f', b'o', b'o', 0, 80],
            "127.0.0.1:443",
        ),
        (
            vec![
                5, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 187,
            ],
            "[::1]:443",
        ),
    ] {
        let mut stream = socks_reply([5, 0], reply, target.parse().unwrap())
            .await
            .unwrap();
        stream.flush().await.unwrap();
        let _ = stream.shutdown().await;
    }
}
#[tokio::test]
async fn socks5_rejects_wrong_versions_methods_refusals_and_invalid_bound_types() {
    for (method, reply) in [
        ([4, 0], vec![]),
        ([5, 3], vec![]),
        ([5, 0], vec![5, 5, 0, 1]),
        ([5, 0], vec![5, 0, 0, 9]),
    ] {
        assert!(
            socks_reply(method, reply, "127.0.0.1:443".parse().unwrap())
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn socks4_ipv6_destination_is_refused_instead_of_truncating_the_address() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let error = connect(
        Some(&proxy("socks4", address)),
        "[::1]:443".parse().unwrap(),
        2,
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("IPv6"));
}
#[tokio::test]
async fn http_proxy_refuses_non_utf8_and_oversized_headers() {
    for response in [b"HTTP/1.1 200 \xff\r\n\r\n".to_vec(), vec![b'x'; 8193]] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = vec![];
            while !header.ends_with(b"\r\n\r\n") {
                header.push(socket.read_u8().await.unwrap());
            }
            let _ = socket.write_all(&response).await;
        });
        assert!(
            connect(
                Some(&proxy("http", address)),
                "127.0.0.1:443".parse().unwrap(),
                2
            )
            .await
            .is_err()
        );
        task.await.unwrap();
    }
}
#[tokio::test]
async fn managed_stream_preserves_writes_flush_and_shutdown_semantics() {
    let (local, mut peer) = tokio::io::duplex(32);
    let task = tokio::spawn(std::future::pending());
    let mut stream = ManagedStream {
        stream: local,
        task,
    };
    stream.write_all(b"owner bytes").await.unwrap();
    stream.flush().await.unwrap();
    let mut bytes = [0; 11];
    peer.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"owner bytes");
    stream.shutdown().await.unwrap();
    assert_eq!(peer.read(&mut [0; 1]).await.unwrap(), 0);
}
