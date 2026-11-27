//! Secret-aware MTProxy obfuscation and Full/padded-intermediate translation.
//! Wire conventions follow Telegram's MTProto obfuscated2 and MTProxy transports.
pub mod faketls;
use super::*;
use aes::cipher::{KeyIvInit, StreamCipher};
use anyhow::{bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

type Cipher = ctr::Ctr128BE<aes::Aes256>;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub struct Secret {
    pub key: [u8; 16],
    pub domain: Option<String>,
}
impl Secret {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= 1024, "MTProxy secret exceeds its byte bound");
        let raw = if text.len().is_multiple_of(2) && text.bytes().all(|b| b.is_ascii_hexdigit()) {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("MTProxy secret encoding is invalid")?
        } else {
            URL_SAFE_NO_PAD
                .decode(text.trim_end_matches('='))
                .or_else(|_| STANDARD_NO_PAD.decode(text.trim_end_matches('=')))
                .context("MTProxy secret encoding is invalid")?
        };
        let (key, domain) = match raw.len() {
            16 => (raw.as_slice(), None),
            17 if raw[0] == 0xdd => (&raw[1..], None),
            length if length >= 18 && raw[0] == 0xee => {
                let domain =
                    std::str::from_utf8(&raw[17..]).context("FakeTLS domain is not valid ASCII")?;
                ensure!(
                    domain.is_ascii()
                        && domain.len() <= 253
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && domain.split('.').all(|label| !label.is_empty()
                            && label.len() <= 63
                            && !label.starts_with('-')
                            && !label.ends_with('-')
                            && label
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
                    "FakeTLS domain is not a valid DNS name"
                );
                (&raw[1..17], Some(domain.to_owned()))
            }
            _ => bail!("MTProxy secret must contain 16 key bytes, dd+key, or ee+key+domain"),
        };
        Ok(Self {
            key: key.try_into()?,
            domain,
        })
    }
}

/// Keystreams intentionally have no Debug implementation: the shared secret
/// and ephemeral AES state must never enter diagnostics.
pub struct Obfuscation {
    pub header: [u8; 64],
    pub(crate) tx: Cipher,
    pub(crate) rx: Cipher,
}
fn ciphers(header: &[u8; 64], secret: &[u8; 16]) -> Result<(Cipher, Cipher)> {
    let tx = Sha256::digest([&header[8..40], secret.as_slice()].concat());
    let reverse: Vec<u8> = header[8..56].iter().rev().copied().collect();
    let rx = Sha256::digest([&reverse[..32], secret.as_slice()].concat());
    Ok((
        Cipher::new_from_slices(&tx, &header[40..56])
            .map_err(|_| anyhow::anyhow!("Invalid MTProxy transmit cipher size"))?,
        Cipher::new_from_slices(&rx, &reverse[32..48])
            .map_err(|_| anyhow::anyhow!("Invalid MTProxy receive cipher size"))?,
    ))
}
impl Obfuscation {
    pub fn new(secret: &Secret, dc: i16) -> Result<Self> {
        ensure!(
            ((1..=5).contains(&dc) || (-5..=-1).contains(&dc)),
            "MTProxy requires a known signed datacenter id"
        );
        let mut header = rand::random::<[u8; 64]>();
        while header[0] == 0xef
            || header[4..8] == [0; 4]
            || [
                b"HEAD",
                b"POST",
                b"GET ",
                b"OPTI",
                b"PVrG",
                &[0xee; 4],
                &[0xdd; 4],
                &[0x16, 3, 1, 2],
            ]
            .iter()
            .any(|prefix| header[..4] == **prefix)
        {
            header = rand::random();
        }
        let (mut tx, rx) = ciphers(&header, &secret.key)?;
        header[56..60].copy_from_slice(&[0xdd; 4]);
        header[60..62].copy_from_slice(&dc.to_le_bytes());
        let mut encrypted = header;
        tx.apply_keystream(&mut encrypted);
        header[56..64].copy_from_slice(&encrypted[56..64]);
        Ok(Self { header, tx, rx })
    }
    /// Decode the peer's handshake for protocol fixtures and local proxy peers.
    pub fn from_client_header(header: [u8; 64], secret: &Secret) -> Result<(Self, i16)> {
        let (mut client_tx, client_rx) = ciphers(&header, &secret.key)?;
        let mut decoded = header;
        client_tx.apply_keystream(&mut decoded);
        ensure!(
            decoded[56..60] == [0xdd; 4],
            "MTProxy obfuscated transport tag or secret is invalid"
        );
        let dc = i16::from_le_bytes(decoded[60..62].try_into()?);
        ensure!(
            ((1..=5).contains(&dc) || (-5..=-1).contains(&dc)),
            "MTProxy datacenter id is invalid"
        );
        Ok((
            Self {
                header,
                tx: client_rx,
                rx: client_tx,
            },
            dc,
        ))
    }
    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.tx.apply_keystream(data)
    }
    pub fn decrypt(&mut self, data: &mut [u8]) {
        self.rx.apply_keystream(data)
    }
}
pub fn full_frame(payload: &[u8], sequence: i32) -> Result<Vec<u8>> {
    ensure!(
        payload.len().is_multiple_of(4) && payload.len() <= MAX_FRAME - 12,
        "Full MTProto payload exceeds its frame bound or alignment"
    );
    let mut frame = Vec::with_capacity(payload.len() + 12);
    frame.extend(((payload.len() + 12) as i32).to_le_bytes());
    frame.extend(sequence.to_le_bytes());
    frame.extend(payload);
    let crc = crc32fast::hash(&frame);
    frame.extend(crc.to_le_bytes());
    Ok(frame)
}
pub async fn read_full<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    sequence: i32,
) -> Result<Vec<u8>> {
    let length = reader.read_i32_le().await?;
    ensure!(
        length >= 12 && (length as usize) <= MAX_FRAME && length % 4 == 0,
        "Full MTProto frame has an invalid length"
    );
    let mut frame = vec![0; length as usize];
    frame[..4].copy_from_slice(&length.to_le_bytes());
    reader.read_exact(&mut frame[4..]).await?;
    ensure!(
        i32::from_le_bytes(frame[4..8].try_into()?) == sequence,
        "Full MTProto frame sequence is invalid"
    );
    let crc = u32::from_le_bytes(frame[frame.len() - 4..].try_into()?);
    ensure!(
        crc32fast::hash(&frame[..frame.len() - 4]) == crc,
        "Full MTProto frame checksum is invalid"
    );
    Ok(frame[8..frame.len() - 4].to_vec())
}

pub(super) async fn connect(
    socket: TcpStream,
    proxy: &Proxy,
    dc: i16,
) -> Result<super::tunnel::Stream> {
    let secret = Secret::parse(
        proxy
            .secret
            .as_deref()
            .context("MTProxy secret is missing")?,
    )?;
    let mut upstream: super::tunnel::Stream = if secret.domain.is_some() {
        faketls::connect(socket, &secret).await?
    } else {
        Box::new(socket)
    };
    let obfuscated = Obfuscation::new(&secret, dc)?;
    upstream.write_all(&obfuscated.header).await?;
    let (client, bridge) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        let _ = translate(bridge, upstream, obfuscated).await;
    });
    Ok(Box::new(super::tunnel::ManagedStream {
        stream: client,
        task,
    }))
}
async fn translate(
    local: tokio::io::DuplexStream,
    upstream: super::tunnel::Stream,
    obfuscated: Obfuscation,
) -> Result<()> {
    let (mut local_read, mut local_write) = tokio::io::split(local);
    let (mut remote_read, mut remote_write) = tokio::io::split(upstream);
    let mut tx = obfuscated.tx;
    let mut rx = obfuscated.rx;
    let outgoing = async {
        let mut sequence = 0i32;
        loop {
            let payload = read_full(&mut local_read, sequence).await?;
            sequence = sequence
                .checked_add(1)
                .context("Full MTProto send sequence exhausted")?;
            let padding = (rand::random::<u8>() % 16) as usize;
            let mut packet = Vec::with_capacity(payload.len() + padding + 4);
            packet.extend(((payload.len() + padding) as u32).to_le_bytes());
            packet.extend(payload);
            for _ in 0..padding {
                packet.push(rand::random())
            }
            tx.apply_keystream(&mut packet);
            remote_write.write_all(&packet).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let incoming = async {
        let mut sequence = 0i32;
        loop {
            let mut header = [0; 4];
            remote_read.read_exact(&mut header).await?;
            rx.apply_keystream(&mut header);
            let length = i32::from_le_bytes(header);
            ensure!(
                length > 0 && (length as usize) <= MAX_FRAME,
                "MTProxy returned an invalid intermediate frame length"
            );
            let mut packet = vec![0; length as usize];
            remote_read.read_exact(&mut packet).await?;
            rx.apply_keystream(&mut packet);
            strip_padding(&mut packet)?;
            if packet.len() == 4 && i32::from_le_bytes(packet.as_slice().try_into()?) < 0 {
                local_write.write_all(&packet).await?;
                bail!("MTProxy returned a transport refusal")
            }
            local_write
                .write_all(&full_frame(&packet, sequence)?)
                .await?;
            sequence = sequence
                .checked_add(1)
                .context("Full MTProto receive sequence exhausted")?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {result=outgoing=>result,result=incoming=>result}
}

/// Current padded-intermediate permits 0..15 bytes. The MTProto envelope, not
/// four-byte rounding, identifies where transport padding ends.
pub fn strip_padding(packet: &mut Vec<u8>) -> Result<()> {
    ensure!(packet.len() >= 4, "MTProxy returned a truncated packet");
    if packet.len() <= 19 && i32::from_le_bytes(packet[..4].try_into()?) < 0 {
        packet.truncate(4);
        return Ok(());
    }
    ensure!(
        packet.len() >= 20,
        "MTProxy returned a truncated MTProto envelope"
    );
    let length = if packet[..8] == [0; 8] {
        let body = i32::from_le_bytes(packet[16..20].try_into()?);
        ensure!(
            body >= 4 && body % 4 == 0,
            "MTProxy unencrypted payload has an invalid length"
        );
        let length = 20usize
            .checked_add(body as usize)
            .context("MTProxy payload length overflow")?;
        ensure!(
            length <= packet.len() && packet.len() - length <= 15,
            "MTProxy unencrypted envelope has invalid padding"
        );
        length
    } else {
        ensure!(packet.len() >= 40, "MTProxy encrypted payload is truncated");
        let padding = (packet.len() - 24) % 16;
        packet.len() - padding
    };
    packet.truncate(length);
    Ok(())
}
