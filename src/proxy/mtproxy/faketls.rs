//! Telegram FakeTLS camouflage, authenticated by the MTProxy shared secret.
//!
//! Adapted from the existing project's proxy_faketls.py and MTProto Proxy
//! Checker (3c1be956812c), itself based on MKultra6969/MK_XRAYchecker.
//!
//! MIT License -- Copyright (c) 2026 MTProto Proxy Checker Contributors.
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
//! of the Software, and to permit persons to whom the Software is furnished to do
//! so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
use super::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const MAX_RECORD: usize = 16408;
const CCS_AND_DATA: &[u8] = &[0x14, 3, 3, 0, 1, 1, 0x17, 3, 3];

pub struct Codec {
    key: [u8; 16],
    domain: String,
    pub session_id: [u8; 32],
    pub random: [u8; 32],
}
fn hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn digest(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32]> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("Invalid FakeTLS HMAC key"))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}
impl Codec {
    pub fn new(secret: &Secret) -> Result<Self> {
        Ok(Self {
            key: secret.key,
            domain: secret
                .domain
                .clone()
                .context("FakeTLS requires an ee secret with a domain")?,
            session_id: [0; 32],
            random: [0; 32],
        })
    }
    pub fn client_hello(&mut self, now: u32) -> Result<Vec<u8>> {
        self.session_id = rand::random();
        let name = self.domain.as_bytes();
        let mut extensions = hex("4a4a0000");
        extensions.extend([0, 0]);
        extensions.extend(((name.len() + 5) as u16).to_be_bytes());
        extensions.extend(((name.len() + 3) as u16).to_be_bytes());
        extensions.push(0);
        extensions.extend((name.len() as u16).to_be_bytes());
        extensions.extend(name);
        extensions.extend(hex("00170000ff01000100000a000a0008baba001d00170018000b00020100002300000010000e000c02683208687474702f312e31000500050100000000000d0012001004030804040105030805050108060601001200000033002b0029baba000100001d0020"));
        extensions.extend(x25519_dalek::x25519(
            rand::random(),
            x25519_dalek::X25519_BASEPOINT_BYTES,
        ));
        extensions.extend(hex(
            "002d00020101002b000b0a9a9a0304030303020301001b00030200021a1a000100",
        ));
        let head = hex("1603010200010001fc0303");
        let mut middle = vec![32];
        middle.extend(self.session_id);
        middle.extend(hex(
            "0020fafa130113021303c02bc02fc02cc030cca9cca8c013c014009c009d002f00350100",
        ));
        let fixed = head.len() + 32 + middle.len() + 2 + extensions.len() + 4;
        ensure!(fixed <= 517, "FakeTLS domain does not fit one ClientHello");
        let padding = 517 - fixed;
        extensions.extend([0, 0x15]);
        extensions.extend((padding as u16).to_be_bytes());
        extensions.resize(extensions.len() + padding, 0);
        let mut hello = head;
        hello.extend([0; 32]);
        hello.extend(middle);
        hello.extend((extensions.len() as u16).to_be_bytes());
        hello.extend(extensions);
        let mut random = digest(&self.key, &[&hello])?;
        for (to, time) in random[28..].iter_mut().zip(now.to_le_bytes()) {
            *to ^= time;
        }
        self.random = random;
        hello[11..43].copy_from_slice(&random);
        ensure!(
            hello.len() == 517,
            "FakeTLS ClientHello construction failed"
        );
        Ok(hello)
    }
    pub fn verify_server_hello(&self, hello: &[u8]) -> Result<()> {
        ensure!(
            hello.len() >= 138
                && hello.starts_with(&[0x16, 3, 3])
                && hello[127..136] == *CCS_AND_DATA,
            "FakeTLS server did not return the expected handshake framing"
        );
        ensure!(
            hello[3..5] == 122u16.to_be_bytes()
                && hello.len() == 138 + u16::from_be_bytes(hello[136..138].try_into()?) as usize,
            "FakeTLS server record lengths do not match the handshake"
        );
        ensure!(
            hello[44..76] == self.session_id,
            "FakeTLS server session id does not match"
        );
        let mut blank = hello.to_vec();
        blank[11..43].fill(0);
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .map_err(|_| anyhow::anyhow!("Invalid FakeTLS HMAC key"))?;
        mac.update(&self.random);
        mac.update(&blank);
        mac.verify_slice(&hello[11..43])
            .map_err(|_| anyhow::anyhow!("FakeTLS server handshake authentication failed"))
    }
}

pub(super) async fn connect(
    mut socket: TcpStream,
    secret: &Secret,
) -> Result<super::super::tunnel::Stream> {
    let mut codec = Codec::new(secret)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let hello = codec.client_hello(
        u32::try_from(now).context("System clock exceeds the FakeTLS timestamp range")?,
    )?;
    socket.write_all(&hello).await?;
    let mut response = vec![0; 138];
    socket.read_exact(&mut response).await?;
    let remaining = u16::from_be_bytes(response[136..138].try_into()?) as usize;
    ensure!(
        remaining <= MAX_RECORD,
        "FakeTLS ServerHello application record exceeds its length bound"
    );
    response.resize(138 + remaining, 0);
    socket.read_exact(&mut response[138..]).await?;
    codec.verify_server_hello(&response)?;
    let (client, bridge) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        let _ = records(bridge, socket).await;
    });
    Ok(Box::new(super::super::tunnel::ManagedStream {
        stream: client,
        task,
    }))
}
async fn records(local: tokio::io::DuplexStream, remote: TcpStream) -> Result<()> {
    let (mut input, mut output) = tokio::io::split(local);
    let (mut network_input, mut network_output) = remote.into_split();
    let outgoing = async {
        // Telegram MTProxy requires exactly one dummy CCS before client data.
        network_output.write_all(&CCS_AND_DATA[..6]).await?;
        let mut data = vec![0; 16384];
        loop {
            let n = input.read(&mut data).await?;
            if n == 0 {
                return Ok::<(), anyhow::Error>(());
            }
            let mut header = vec![0x17, 3, 3];
            header.extend((n as u16).to_be_bytes());
            network_output.write_all(&header).await?;
            network_output.write_all(&data[..n]).await?;
        }
    };
    let incoming = async {
        let mut empty = 0;
        loop {
            let mut header = [0; 5];
            network_input.read_exact(&mut header).await?;
            ensure!(
                matches!(header[0], 0x14 | 0x17) && header[1..3] == [3, 3],
                "FakeTLS proxy returned a non-data TLS record"
            );
            let n = u16::from_be_bytes(header[3..5].try_into()?) as usize;
            ensure!(n <= MAX_RECORD, "FakeTLS record exceeds its byte bound");
            let mut data = vec![0; n];
            network_input.read_exact(&mut data).await?;
            if header[0] == 0x14 {
                ensure!(data == [1], "Invalid FakeTLS change-cipher-spec record");
                empty += 1;
                ensure!(
                    empty <= 128,
                    "FakeTLS proxy returned too many consecutive control records"
                );
                continue;
            }
            if n == 0 {
                empty += 1;
                ensure!(
                    empty <= 128,
                    "FakeTLS proxy returned too many empty records"
                );
                continue;
            }
            empty = 0;
            output.write_all(&data).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {result=outgoing=>result,result=incoming=>result}
}
#[cfg(test)]
#[path = "../../../tests/unit/proxy_faketls.rs"]
mod tests;
