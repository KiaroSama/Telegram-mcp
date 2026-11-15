//! Protocol negotiations to a TCP proxy. These functions do not send account data.
use super::*;
use anyhow::{bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::TcpStream,
};

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type Stream = Box<dyn Io>;

pub async fn connect(proxy: Option<&Proxy>, target: SocketAddr, dc: i16) -> Result<Stream> {
    let Some(proxy) = proxy else {
        return Ok(Box::new(
            TcpStream::connect(target)
                .await
                .context("Direct Telegram connection failed")?,
        ));
    };
    super::pools::validate(proxy)?;
    let mut socket = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .context("Proxy endpoint could not be reached")?;
    match proxy.kind.as_str() {
        "socks5" => socks5(&mut socket, proxy, target).await?,
        "socks4" => socks4(&mut socket, proxy, target).await?,
        "http" => http(&mut socket, proxy, target).await?,
        "mtproto" => return super::mtproxy::connect(socket, proxy, dc).await,
        _ => bail!("Unsupported configured proxy protocol"),
    }
    Ok(Box::new(socket))
}
async fn socks5(socket: &mut TcpStream, proxy: &Proxy, target: SocketAddr) -> Result<()> {
    let authenticated = proxy.username.as_ref().is_some_and(|s| !s.is_empty())
        || proxy.password.as_ref().is_some_and(|s| !s.is_empty());
    socket
        .write_all(if authenticated {
            &[5, 2, 0, 2]
        } else {
            &[5, 1, 0]
        })
        .await?;
    let mut method = [0; 2];
    socket.read_exact(&mut method).await?;
    ensure!(
        method[0] == 5,
        "Proxy returned an invalid SOCKS5 negotiation"
    );
    match method[1] {
        0 => {}
        2 if authenticated => {
            let username = proxy.username.as_deref().unwrap_or("").as_bytes();
            let password = proxy.password.as_deref().unwrap_or("").as_bytes();
            ensure!(
                username.len() <= 255 && password.len() <= 255,
                "SOCKS5 credentials exceed 255 bytes"
            );
            let mut auth = vec![1, username.len() as u8];
            auth.extend(username);
            auth.push(password.len() as u8);
            auth.extend(password);
            socket.write_all(&auth).await?;
            let mut result = [0; 2];
            socket.read_exact(&mut result).await?;
            ensure!(result == [1, 0], "Proxy refused SOCKS5 authentication");
        }
        _ => bail!("Proxy offered an unsupported SOCKS5 authentication method"),
    }
    let mut request = vec![5, 1, 0];
    append_address(&mut request, target);
    socket.write_all(&request).await?;
    let mut reply = [0; 4];
    socket.read_exact(&mut reply).await?;
    ensure!(
        reply[0] == 5 && reply[1] == 0 && reply[2] == 0,
        "Proxy refused the SOCKS5 destination"
    );
    read_bound(socket, reply[3]).await?;
    Ok(())
}
async fn socks4(socket: &mut TcpStream, proxy: &Proxy, target: SocketAddr) -> Result<()> {
    let IpAddr::V4(ip) = target.ip() else {
        bail!("SOCKS4 cannot route an IPv6 Telegram destination")
    };
    let user = proxy.username.as_deref().unwrap_or("").as_bytes();
    ensure!(
        user.len() <= 255 && !user.contains(&0),
        "Invalid SOCKS4 user id"
    );
    let mut request = vec![4, 1];
    request.extend(target.port().to_be_bytes());
    request.extend(ip.octets());
    request.extend(user);
    request.push(0);
    socket.write_all(&request).await?;
    let mut result = [0; 8];
    socket.read_exact(&mut result).await?;
    ensure!(
        matches!(result[0], 0 | 4) && result[1] == 90,
        "Proxy refused the SOCKS4 destination"
    );
    Ok(())
}
async fn http(socket: &mut TcpStream, proxy: &Proxy, target: SocketAddr) -> Result<()> {
    let authority = target.to_string();
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if proxy.username.is_some() || proxy.password.is_some() {
        let credentials = format!(
            "{}:{}",
            proxy.username.as_deref().unwrap_or(""),
            proxy.password.as_deref().unwrap_or("")
        );
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            STANDARD.encode(credentials)
        ));
    }
    request.push_str("\r\n");
    socket.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        ensure!(
            response.len() < 8192,
            "HTTP proxy response headers exceed 8 KiB"
        );
        response.push(socket.read_u8().await?);
    }
    let line = std::str::from_utf8(&response)
        .context("HTTP proxy response is not valid text")?
        .lines()
        .next()
        .unwrap_or("");
    let parts: Vec<&str> = line.split_whitespace().collect();
    ensure!(
        parts.len() >= 2 && matches!(parts[0], "HTTP/1.0" | "HTTP/1.1") && parts[1] == "200",
        "HTTP proxy refused the CONNECT tunnel"
    );
    Ok(())
}
pub(crate) fn append_address(request: &mut Vec<u8>, target: SocketAddr) {
    match target.ip() {
        IpAddr::V4(ip) => {
            request.push(1);
            request.extend(ip.octets());
        }
        IpAddr::V6(ip) => {
            request.push(4);
            request.extend(ip.octets());
        }
    }
    request.extend(target.port().to_be_bytes());
}
pub(crate) async fn read_bound(socket: &mut TcpStream, kind: u8) -> Result<()> {
    let bytes = match kind {
        1 => 4,
        4 => 16,
        3 => socket.read_u8().await? as usize,
        _ => bail!("Invalid SOCKS5 address type"),
    };
    let mut discard = vec![0; bytes + 2];
    socket.read_exact(&mut discard).await?;
    Ok(())
}

pub(crate) struct ManagedStream {
    pub stream: tokio::io::DuplexStream,
    pub task: JoinHandle<()>,
}
impl Drop for ManagedStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl AsyncRead for ManagedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}
impl AsyncWrite for ManagedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
#[cfg(test)]
#[path = "../../tests/unit/proxy_tunnel.rs"]
mod tests;
