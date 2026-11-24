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

#[tokio::test]
async fn remote_plaintext_credentials_are_refused_before_any_protocol_bytes() {
    let route = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect("192.0.2.1:9").unwrap(); // Select a local interface; no datagram is sent.
    let ip = route.local_addr().unwrap().ip();
    assert!(!ip.is_loopback());
    for kind in ["http", "socks5", "socks4"] {
        let listener = TcpListener::bind((ip, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut proxy = proxy(kind, address);
        proxy.username = Some("synthetic-user".into());
        let connection = connect(Some(&proxy), "149.154.167.50:443".parse().unwrap(), 2);
        let (result, accepted) = tokio::join!(connection, listener.accept());
        assert!(result.err().unwrap().to_string().contains("verified HTTPS"));
        let mut socket = accepted.unwrap().0;
        assert!(
            socket.read_u8().await.is_err(),
            "No credential or greeting bytes may reach this socket"
        );
    }
}

#[tokio::test]
async fn https_connect_requires_a_trusted_certificate_and_matching_server_name() {
    for (trusted, matching) in [(true, true), (false, true), (true, false)] {
        let material = rcgen::generate_simple_self_signed(vec![
            if matching {
                "127.0.0.1"
            } else {
                "wrong.example"
            }
            .into(),
        ])
        .unwrap();
        let certificate = material.cert.der().clone();
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(material.signing_key.serialize_der())
                    .into(),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let Ok(mut socket) = tokio_rustls::TlsAcceptor::from(Arc::new(server))
                .accept(socket)
                .await
            else {
                return None;
            };
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.ok()?);
            }
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nprivate tunnel bytes")
                .await
                .ok()?;
            Some(String::from_utf8(request).unwrap())
        });
        let mut roots = rustls::RootCertStore::empty();
        if trusted {
            roots.add(certificate).unwrap();
        }
        let config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let mut proxy = proxy("https", address);
        proxy.username = Some("synthetic-user".into());
        proxy.password = Some("synthetic-password".into());
        let outcome = connect_verified(
            Some(&proxy),
            "149.154.167.50:443".parse().unwrap(),
            2,
            Some(config),
        )
        .await;
        if trusted && matching {
            let mut stream = outcome.unwrap();
            let mut bytes = [0; 20];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"private tunnel bytes");
            let request = task.await.unwrap().unwrap();
            assert!(request.starts_with("CONNECT 149.154.167.50:443 HTTP/1.1\r\n"));
            assert!(request.contains("Proxy-Authorization: Basic "));
        } else {
            assert!(
                outcome
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("TLS handshake verification failed")
            );
            assert!(
                task.await.unwrap().is_none(),
                "CONNECT and credentials must not be sent before certificate verification"
            );
        }
    }
}
