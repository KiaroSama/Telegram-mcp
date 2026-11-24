//! Independent transport review fixtures. Only loopback sockets and synthetic
//! bytes are used; these tests never contain or request Telegram auth keys.
use grammers_mtproto::{DequeBuffer, transport::Transport};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::{net::SocketAddr, time::Duration};
use telegram_mcp::{
    proxy::{
        self,
        mtproxy::{self, Obfuscation, Secret, faketls::Codec},
    },
    tools::local::proxies::Proxy,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const KEY: [u8; 16] = [
    0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
const WAIT: Duration = Duration::from_secs(3);
fn key_hex() -> String {
    KEY.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn mtproxy(address: SocketAddr, secret: String) -> Proxy {
    Proxy {
        kind: "mtproto".into(),
        host: address.ip().to_string(),
        port: address.port(),
        secret: Some(secret),
        username: None,
        password: None,
    }
}

fn ee_secret() -> Secret {
    Secret {
        key: KEY,
        domain: Some("www.example.com".into()),
    }
}

fn ee_text() -> String {
    format!(
        "ee{}{}",
        key_hex(),
        b"www.example.com"
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>()
    )
}

struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[test]
fn mtproxy_matches_external_aes_ctr_vector_and_signed_media_dc() {
    // Computed independently with OpenSSL AES-256-CTR and SHA256. Plain header
    // is bytes0..63 with dddddddd at56 and signed DC-4 at60. Transmit encrypts
    // the64-byte header before any application bytes; receive starts at0.
    let header = bytes(concat!(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "202122232425262728292a2b2c2d2e2f30313233343536378f3d4055c17444b1"
    ));
    let secret = Secret::parse(&key_hex()).unwrap();
    let (mut server, dc) =
        Obfuscation::from_client_header(header.try_into().unwrap(), &secret).unwrap();
    assert_eq!(dc, -4);
    let mut outgoing = bytes("ff052967decb9cfaf40ebdd01f5e5c7dce864c54e82ece458137d04ac59deecd");
    server.decrypt(&mut outgoing[..1]);
    server.decrypt(&mut outgoing[1..19]);
    server.decrypt(&mut outgoing[19..]);
    assert_eq!(outgoing, b"fixed independent outgoing bytes");
    let mut incoming = b"fixed independent incoming bytes".to_vec();
    server.encrypt(&mut incoming);
    assert_eq!(
        incoming,
        bytes("6e8895ba6e5385e2e17e83496308a52aa83246cd97cccc32863f65c6e837ea07")
    );
}

#[test]
fn full_transport_matches_pinned_grammers_pack_and_unpack() {
    let mut sender = grammers_mtproto::transport::Full::new();
    let mut receiver = grammers_mtproto::transport::Full::new();
    for (sequence, payload) in [b"".as_slice(), b"1234", b"0123456789abcdef"]
        .into_iter()
        .enumerate()
    {
        let mut buffer = DequeBuffer::with_capacity(payload.len(), 0);
        buffer.extend_back(payload);
        sender.pack(&mut buffer);
        let mut frame = mtproxy::full_frame(payload, sequence as i32).unwrap();
        assert_eq!(frame.as_slice(), buffer.as_ref());
        let offset = receiver.unpack(&mut frame).unwrap();
        assert_eq!(&frame[offset.data_range], payload);
        assert_eq!(offset.next_offset, payload.len() + 12);
    }
    // Published grammers empty-frame CRC vector, independent of its runtime.
    assert_eq!(
        mtproxy::full_frame(&[], 0).unwrap(),
        [12, 0, 0, 0, 0, 0, 0, 0, 38, 202, 141, 50]
    );
}

fn plain(payload: &[u8]) -> Vec<u8> {
    let mut message = vec![0; 8];
    message.extend(1u64.to_le_bytes());
    message.extend((payload.len() as u32).to_le_bytes());
    message.extend(payload);
    message
}

async fn response_stream(payload: Vec<u8>, padding: usize) -> (proxy::tunnel::Stream, Task) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = Task(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = [0; 64];
        socket.read_exact(&mut header).await.unwrap();
        let secret = Secret::parse(&key_hex()).unwrap();
        let (mut codec, _) = Obfuscation::from_client_header(header, &secret).unwrap();
        let mut packet = ((payload.len() + padding) as u32).to_le_bytes().to_vec();
        packet.extend(payload);
        packet.extend(std::iter::repeat_n(0xa5, padding));
        codec.encrypt(&mut packet);
        // Deliberately fragment length and body across arbitrary TCP writes.
        for chunk in packet.chunks(7) {
            socket.write_all(chunk).await.unwrap();
        }
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    }));
    let route = mtproxy(address, key_hex());
    let stream = proxy::tunnel::connect(Some(&route), "149.154.167.50:443".parse().unwrap(), 2)
        .await
        .unwrap();
    (stream, task)
}

#[tokio::test]
async fn all_sixteen_spec_padding_lengths_are_removed_from_plain_packets() {
    // Telegram specifies0..15 bytes, rather than the old Telethon0..3 shortcut:
    // https://core.telegram.org/mtproto/mtproto-transports#padded-intermediate
    let payload = plain(b"synthetic TL object!");
    assert_eq!(payload.len() % 4, 0);
    for padding in 0..16 {
        let (mut stream, _task) = response_stream(payload.clone(), padding).await;
        let received = tokio::time::timeout(WAIT, mtproxy::read_full(&mut stream, 0))
            .await
            .expect("padding decoder stalled")
            .unwrap();
        assert_eq!(received, payload, "plain packet padding={padding}");
    }
}

#[tokio::test]
async fn all_sixteen_spec_padding_lengths_preserve_encrypted_packet_alignment() {
    // Synthetic auth-key ID + message-key + three AES blocks. None is a real
    // owner key; the gateway translates framing without decrypting MTProto.
    let mut payload = (1u8..=24).collect::<Vec<_>>();
    payload.extend([0x72; 48]);
    assert_eq!((payload.len() - 24) % 16, 0);
    for padding in 0..16 {
        let (mut stream, _task) = response_stream(payload.clone(), padding).await;
        let received = tokio::time::timeout(WAIT, mtproxy::read_full(&mut stream, 0))
            .await
            .expect("padding decoder stalled")
            .unwrap();
        assert_eq!(received, payload, "encrypted packet padding={padding}");
    }
}

#[tokio::test]
async fn padded_transport_refusal_is_forwarded_as_a_full_transport_status() {
    for padding in 0..16 {
        let (mut stream, _task) = response_stream((-404i32).to_le_bytes().to_vec(), padding).await;
        let status = tokio::time::timeout(WAIT, stream.read_i32_le())
            .await
            .expect("transport status stalled")
            .unwrap();
        assert_eq!(status, -404, "transport-error padding={padding}");
    }
}

fn authenticated_server_hello(codec: &Codec) -> Vec<u8> {
    let mut hello = vec![0; 142];
    hello[..5].copy_from_slice(&[0x16, 3, 3, 0, 122]);
    hello[43] = 32;
    hello[44..76].copy_from_slice(&codec.session_id);
    hello[127..138].copy_from_slice(&[0x14, 3, 3, 0, 1, 1, 0x17, 3, 3, 0, 4]);
    hello[138..].copy_from_slice(b"test");
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    mac.update(&codec.random);
    mac.update(&hello);
    hello[11..43].copy_from_slice(&mac.finalize().into_bytes());
    hello
}

#[test]
fn fake_tls_server_authentication_cannot_be_replayed_for_a_new_client_nonce() {
    let mut first = Codec::new(&ee_secret()).unwrap();
    let mut second = Codec::new(&ee_secret()).unwrap();
    first.client_hello(1770000000).unwrap();
    second.client_hello(1770000000).unwrap();
    let hello = authenticated_server_hello(&first);
    first.verify_server_hello(&hello).unwrap();
    assert_ne!(first.session_id, second.session_id);
    assert_ne!(first.random, second.random);
    assert!(second.verify_server_hello(&hello).is_err());
    // Rebinding only the public session ID does not repair the HMAC challenge.
    second.session_id = first.session_id;
    assert!(second.verify_server_hello(&hello).is_err());
}

#[tokio::test]
async fn oversized_fake_tls_server_record_is_rejected_before_body_read() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _task = Task(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut client = [0; 517];
        socket.read_exact(&mut client).await.unwrap();
        let mut header = vec![0; 138];
        header[..5].copy_from_slice(&[0x16, 3, 3, 0, 122]);
        header[136..138].copy_from_slice(&16409u16.to_be_bytes());
        socket.write_all(&header).await.unwrap();
        // Keep the socket open without a body: a correct bound fails now.
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    }));
    let p = mtproxy(address, ee_text());
    let result = tokio::time::timeout(
        WAIT,
        proxy::tunnel::connect(Some(&p), "149.154.167.50:443".parse().unwrap(), 2),
    )
    .await
    .expect("FakeTLS tried to read an oversized body");
    let error = result.err().expect("oversized FakeTLS record was accepted");
    assert!(error.to_string().contains("length bound"));
}

#[tokio::test]
async fn dropping_mtproxy_stream_closes_its_active_upstream_task() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, received) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = [0; 64];
        socket.read_exact(&mut header).await.unwrap();
        ready.send(()).unwrap();
        let mut byte = [0];
        socket.read(&mut byte).await.unwrap()
    });
    let p = mtproxy(address, key_hex());
    let stream = proxy::tunnel::connect(Some(&p), "149.154.167.50:443".parse().unwrap(), 2)
        .await
        .unwrap();
    received.await.unwrap();
    drop(stream);
    assert_eq!(
        tokio::time::timeout(WAIT, server)
            .await
            .expect("dropped MTProxy stream retained its network task")
            .unwrap(),
        0
    );
}
