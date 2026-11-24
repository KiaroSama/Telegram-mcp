#[tokio::test]
async fn cancelling_tls_server_scope_aborts_the_owned_signal_waiter() {
    use tokio::io::AsyncReadExt;

    struct Finished(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Finished {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    let (started, running) = tokio::sync::oneshot::channel();
    let (finished, stopped) = tokio::sync::oneshot::channel();
    let material = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        material.cert.pem().into_bytes(),
        material.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = axum_server::Handle::new();
    let serving = tokio::spawn(
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(handle.clone())
            .serve(axum::Router::new().into_make_service()),
    );
    let address = tokio::time::timeout(Duration::from_secs(5), handle.listening())
        .await
        .unwrap()
        .unwrap();
    // This connection remains inside the TLS handshake until cancellation.
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.connection_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let sessions = tokio_util::sync::CancellationToken::new();
    let signal = super::http::ShutdownSignal::new(
        tokio::spawn(async move {
            let _finished = Finished(Some(finished));
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }),
        handle.clone(),
        sessions.clone(),
    );
    running.await.unwrap();
    drop(signal);
    tokio::time::timeout(Duration::from_secs(5), stopped)
        .await
        .expect("Cancelled TLS server retained its signal task")
        .unwrap();
    assert!(sessions.is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .expect("Cancelled TLS server retained its listener")
        .unwrap()
        .unwrap();
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
        .await
        .expect("Cancelled TLS handshake retained its socket");
    assert!(match closed {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        ),
        _ => false,
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.connection_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Cancelled TLS server retained a connection task");
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}

static QR_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn local_qr_yields_before_rendering_and_publishes_without_a_telegram_account() {
    use base64::Engine;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let _serial = QR_TEST_SERIAL.lock().await;
        let (mcp, api, _temporary) = service(Value::Null, access::AccessPolicy::default());
        mcp.runtime.accounts.lock().await.clear();
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (started, running) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            let _ = blocked.recv();
        });
        running.await.unwrap();
        let link = "https://t.me/+owned-qr-publication";
        let mut operation = Box::pin(mcp.execute("get_invite_link_qr", json!({"link":link})));
        assert!(
            futures_util::poll!(operation.as_mut()).is_pending(),
            "QR rendering and synchronous publication ran inside the first async poll"
        );
        release.send(()).unwrap();
        blocker.await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), operation)
            .await
            .unwrap()
            .unwrap();
        let path = std::path::Path::new(result["path"].as_str().unwrap());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result["content"][0]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(result["content"][0]["mimeType"], "image/png");
        assert_eq!(result["metadata"]["link"], link);
        assert_eq!(result["metadata"]["path"], result["path"]);
        assert!(path.starts_with(mcp.runtime.state.default_downloads().unwrap()));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!(image.width(), image.height());
        assert!(mcp.runtime.accounts.lock().await.is_empty());
        assert!(api.calls.lock().unwrap().is_empty());
    });
}

#[test]
fn local_qr_cancelled_callers_retain_bounded_admission_until_owned_publication_finishes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let _serial = QR_TEST_SERIAL.lock().await;
        let (mcp, api, _temporary) = service(Value::Null, access::AccessPolicy::default());
        mcp.runtime.accounts.lock().await.clear();
        let capacity = QR_WORKERS.available_permits();
        assert!(capacity > 0);
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (started, running) = tokio::sync::oneshot::channel();
        // The gate sender lives inside this future: every panic drops it before
        // Runtime shutdown waits for the worker, so no assertion can deadlock.
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            let _ = blocked.recv();
        });
        running.await.unwrap();
        let mut requests = (0..capacity)
            .map(|index| {
                Box::pin(mcp.execute(
                    "get_invite_link_qr",
                    json!({"link":format!("https://t.me/+cancelled{index}")}),
                ))
            })
            .collect::<Vec<_>>();
        for request in &mut requests {
            assert!(futures_util::poll!(request.as_mut()).is_pending());
        }
        assert_eq!(QR_WORKERS.available_permits(), 0);
        drop(requests);
        assert!(
            QR_WORKERS.try_acquire().is_err(),
            "Cancelling callers released admission while their publication work was queued"
        );

        let mut waiting = Box::pin(mcp.execute(
            "get_invite_link_qr",
            json!({"link":"https://t.me/+must-not-start"}),
        ));
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(56)).await;
        let error = waiting.await.unwrap_err();
        assert!(error.to_string().contains("Tool exceeded its time budget"));
        assert!(QR_WORKERS.try_acquire().is_err());

        release.send(()).unwrap();
        blocker.await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let _completed = loop {
            if let Ok(permits) = QR_WORKERS.try_acquire_many(capacity as u32) {
                break permits;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "QR workers retained admission after their owned work completed"
            );
            tokio::task::yield_now().await;
        };
        let paths = std::fs::read_dir(mcp.runtime.state.default_downloads().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(
            paths.len(),
            capacity,
            "A timed-out waiter dispatched extra work or a cancelled worker lost its publication"
        );
        for path in paths {
            assert_eq!(
                path.extension().and_then(|value| value.to_str()),
                Some("png"),
                "A QR worker orphaned its staging file"
            );
            image::load_from_memory(&std::fs::read(path).unwrap()).unwrap();
        }
        assert!(mcp.runtime.accounts.lock().await.is_empty());
        assert!(api.calls.lock().unwrap().is_empty());
    });
}

#[tokio::test]
async fn dropping_plaintext_serve_cancels_already_initialized_mcp_sessions() {
    use rmcp::transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    };
    let (server, _api, _temporary) = service(Value::Null, Default::default());
    let state = Arc::clone(&server.runtime.state);
    let serving = tokio::spawn(super::http::serve(server, "127.0.0.1:0".parse().unwrap()));
    let record = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = state.read_json("http-service.json", Value::Null).unwrap();
            if record["url"].is_string() {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        reqwest::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(record["url"].as_str().unwrap().to_owned())
            .auth_header(record["token"].as_str().unwrap().to_owned()),
    );
    let remote = tokio::time::timeout(Duration::from_secs(5), ().serve(transport))
        .await
        .unwrap()
        .unwrap();
    assert!(!remote.list_all_tools().await.unwrap().is_empty());
    serving.abort();
    assert!(serving.await.unwrap_err().is_cancelled());
    let result = tokio::time::timeout(Duration::from_secs(2), remote.list_all_tools()).await;
    assert!(
        !matches!(result, Ok(Ok(_))),
        "An initialized MCP session kept dispatching after its server scope was dropped"
    );
    let _ = remote.cancel().await;
}
