use super::support::*;
use anyhow::{Result, ensure};
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::net::Ipv4Addr;
use telegram_mcp::proxy::{
    self,
    mtproxy::{self, Obfuscation, Secret, faketls::Codec},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
const KEY: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
fn secret() -> String {
    let mut raw = vec![0xee];
    raw.extend(KEY);
    raw.extend(b"www.example.com");
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hello(codec: &Codec, tampered: bool) -> Vec<u8> {
    let mut server = vec![0; 142];
    server[..3].copy_from_slice(&[0x16, 3, 3]);
    server[3..5].copy_from_slice(&122u16.to_be_bytes());
    server[43] = 32;
    server[44..76].copy_from_slice(&codec.session_id);
    server[127..136].copy_from_slice(&[0x14, 3, 3, 0, 1, 1, 0x17, 3, 3]);
    server[136..138].copy_from_slice(&4u16.to_be_bytes());
    server[138..].copy_from_slice(b"data");
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    mac.update(&codec.random);
    mac.update(&server);
    let digest = mac.finalize().into_bytes();
    server[11..43].copy_from_slice(&digest);
    if tampered {
        server[11] ^= 1
    }
    server
}
#[test]
fn fake_tls_clienthello_is517bytes_and_includes_sni_domain() {
    let mut codec = Codec::new(&Secret::parse(&secret()).unwrap()).unwrap();
    let packet = codec.client_hello(1770000000).unwrap();
    assert_eq!(packet.len(), 517);
    assert_eq!(&packet[..3], &[0x16, 3, 1]);
    assert!(packet.windows(15).any(|part| part == b"www.example.com"));
}
#[test]
fn fake_tls_random_authenticates_hello_and_encodes_timestamp() {
    let mut codec = Codec::new(&Secret::parse(&secret()).unwrap()).unwrap();
    let timestamp = 1770000000u32;
    let packet = codec.client_hello(timestamp).unwrap();
    let mut blank = packet.clone();
    blank[11..43].fill(0);
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    mac.update(&blank);
    let digest = mac.finalize().into_bytes();
    assert_eq!(&packet[11..39], &digest[..28]);
    let stamp: Vec<u8> = packet[39..43]
        .iter()
        .zip(&digest[28..])
        .map(|(a, b)| a ^ b)
        .collect();
    assert_eq!(stamp, timestamp.to_le_bytes());
}
#[test]
fn fake_tls_server_hmac_and_session_binding_are_both_required() {
    let mut codec = Codec::new(&Secret::parse(&secret()).unwrap()).unwrap();
    codec.client_hello(1770000000).unwrap();
    assert!(codec.verify_server_hello(&hello(&codec, false)).is_ok());
    assert!(codec.verify_server_hello(&hello(&codec, true)).is_err());
    let mut wrong = hello(&codec, false);
    wrong[44] ^= 1;
    assert!(
        codec
            .verify_server_hello(&wrong)
            .unwrap_err()
            .to_string()
            .contains("session")
    );
}
#[test]
fn fake_tls_rejects_invalid_server_framing() {
    let mut codec = Codec::new(&Secret::parse(&secret()).unwrap()).unwrap();
    codec.client_hello(1770000000).unwrap();
    let mut wrong = hello(&codec, false);
    wrong[127] = 0x16;
    assert!(codec.verify_server_hello(&wrong).is_err());
    assert!(codec.verify_server_hello(b"short").is_err());
}
#[test]
fn standard_and_url_safe_base64_secrets_both_work() {
    let key = [251u8; 16];
    for secret in [
        base64::engine::general_purpose::STANDARD.encode(key),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key),
    ] {
        assert_eq!(Secret::parse(&secret).unwrap().key, key);
    }
}
#[test]
fn fake_tls_requires_dns_domain_after_key() {
    let mut no_domain = vec![0xee];
    no_domain.extend(KEY);
    assert!(
        Secret::parse(
            &no_domain
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
        .is_err()
    );
    let invalid = format!(
        "ee{}{}",
        KEY.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        b"bad domain"
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert!(Secret::parse(&invalid).is_err());
}

async fn record_data(
    socket: &mut TcpStream,
    buffer: &mut Vec<u8>,
    count: usize,
) -> Result<Vec<u8>> {
    while buffer.len() < count {
        let mut header = [0; 5];
        socket.read_exact(&mut header).await?;
        ensure!(header[..3] == [0x17, 3, 3], "invalid application record");
        let n = u16::from_be_bytes(header[3..5].try_into()?) as usize;
        ensure!(n <= 16408, "oversized TLS record");
        let mut data = vec![0; n];
        socket.read_exact(&mut data).await?;
        buffer.extend(data);
    }
    Ok(buffer.drain(..count).collect())
}
async fn server(tampered: bool) -> (std::net::SocketAddr, Task) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};children.spawn(async move{let result:Result<()>=async{
                let mut client=vec![0;517];socket.read_exact(&mut client).await?;let mut codec=Codec::new(&Secret::parse(&secret())?)?;codec.session_id.copy_from_slice(&client[44..76]);codec.random.copy_from_slice(&client[11..43]);
                let mut blank=client.clone();blank[11..43].fill(0);let mut mac=Hmac::<Sha256>::new_from_slice(&KEY)?;mac.update(&blank);ensure!(mac.finalize().into_bytes()[..28]==client[11..39],"client handshake digest is invalid");socket.write_all(&hello(&codec,tampered)).await?;if tampered{return Ok(())}
                let mut ccs = [0;6]; socket.read_exact(&mut ccs).await?; ensure!(ccs == [0x14,3,3,0,1,1], "missing client CCS required by official MTProxy"); let mut buffer=vec![];let head:[u8;64]=record_data(&mut socket,&mut buffer,64).await?.try_into().unwrap();let (mut cipher,dc)=Obfuscation::from_client_header(head,&Secret::parse(&secret())?)?;ensure!(dc==2,"wrong datacenter");loop{let mut length=record_data(&mut socket,&mut buffer,4).await?;cipher.decrypt(&mut length);let n=u32::from_le_bytes(length.try_into().unwrap()) as usize;ensure!(n<=mtproxy::MAX_FRAME,"large MTProxy frame");let mut payload=record_data(&mut socket,&mut buffer,n).await?;cipher.decrypt(&mut payload);ensure!(payload.len()>=20&&payload[..8]==[0;8],"expected unencrypted probe");let declared=20+u32::from_le_bytes(payload[16..20].try_into().unwrap()) as usize;ensure!(declared<=payload.len()&&payload.len()-declared<=15,"invalid padded probe length");payload.truncate(declared);let response=reply(&payload,false)?;let mut encrypted=(response.len() as u32).to_le_bytes().to_vec();encrypted.extend(response);cipher.encrypt(&mut encrypted);let mut header=vec![0x17,3,3];header.extend((encrypted.len() as u16).to_be_bytes());socket.write_all(&header).await?;socket.write_all(&encrypted).await?;}
            }.await;let _=result;});},Some(_)=children.join_next(),if !children.is_empty()=>{}}
        }
    });
    (address, Task(task))
}
#[tokio::test]
async fn ee_proxy_completes_hmac_and_obfuscated_transport_probe() {
    let (address, _server) = server(false).await;
    let mut p = proxy("mtproto", address);
    p.secret = Some(secret());
    proxy::probe(&p, "149.154.167.50:443".parse().unwrap(), 2)
        .await
        .unwrap();
}
#[tokio::test]
async fn forged_ee_server_hello_is_refused_before_mtproto_payload() {
    let (address, _server) = server(true).await;
    let mut p = proxy("mtproto", address);
    p.secret = Some(secret());
    assert!(
        proxy::probe(&p, "149.154.167.50:443".parse().unwrap(), 2)
            .await
            .unwrap_err()
            .to_string()
            .contains("authentication")
    );
}
