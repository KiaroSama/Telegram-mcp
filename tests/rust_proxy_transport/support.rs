#![allow(dead_code)]
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};
use telegram_mcp::{
    account_manager::ConfigSnapshot,
    proxy::{
        self,
        mtproxy::{self, Obfuscation, Secret},
    },
    state::State,
    tools::local::proxies::Proxy,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
pub struct Task(pub JoinHandle<()>);
pub fn synthetic_secret(byte: u8) -> String {
    [byte; 16].iter().map(|b| format!("{b:02x}")).collect()
}
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub fn proxy(kind: &str, address: SocketAddr) -> Proxy {
    Proxy {
        kind: kind.into(),
        host: address.ip().to_string(),
        port: address.port(),
        secret: None,
        username: None,
        password: None,
    }
}
pub fn snapshot(values: &[(&str, &str)]) -> ConfigSnapshot {
    ConfigSnapshot {
        values: values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        digest: "fixture".into(),
        path: std::path::PathBuf::from("fixture.env"),
        process_keys: BTreeSet::new(),
    }
}
pub fn state() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    (temp, state)
}
pub fn reply(request: &[u8], wrong: bool) -> Result<Vec<u8>> {
    if request.len() >= 40
        && request[..8] == [0; 8]
        && request[20..24] == 0xbe7e8ef1u32.to_le_bytes()
    {
        let mut body = 0x05162463u32.to_le_bytes().to_vec();
        let mut nonce = request[24..40].to_vec();
        if wrong {
            nonce[0] ^= 1
        }
        body.extend(nonce);
        body.extend([9; 16]);
        body.extend([8, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0]);
        body.extend(0x1cb5c415u32.to_le_bytes());
        body.extend(1i32.to_le_bytes());
        body.extend(12345i64.to_le_bytes());
        let mut response = vec![0; 8];
        response.extend(1u64.to_le_bytes());
        response.extend((body.len() as u32).to_le_bytes());
        response.extend(body);
        Ok(response)
    } else {
        Ok(request.to_vec())
    }
}
pub async fn telegram(wrong: bool) -> (SocketAddr, Task, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let recorded = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};let recorded=Arc::clone(&recorded);children.spawn(async move{for sequence in 0..100{let Ok(request)=mtproxy::read_full(&mut socket,sequence).await else{break};recorded.lock().unwrap().push(request.clone());let response=reply(&request,wrong).unwrap();let response=mtproxy::full_frame(&response,sequence).unwrap();if socket.write_all(&response).await.is_err(){break}}});},Some(_)=children.join_next(),if !children.is_empty()=>{}}
        }
    });
    (address, Task(task), seen)
}
pub async fn http_proxy(
    destination: SocketAddr,
    refuse: bool,
) -> (SocketAddr, Task, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let recorded = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};let recorded=Arc::clone(&recorded);children.spawn(async move{let mut request=vec![];while !request.ends_with(b"\r\n\r\n")&&request.len()<8192{let Ok(byte)=socket.read_u8().await else{return};request.push(byte);}recorded.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());if refuse{let _=socket.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;return}let Ok(mut upstream)=TcpStream::connect(destination).await else{return};if socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_ok(){let _=tokio::io::copy_bidirectional(&mut socket,&mut upstream).await;}});},Some(_)=children.join_next(),if !children.is_empty()=>{}}
        }
    });
    (address, Task(task), seen)
}
pub async fn socks_proxy(
    destination: SocketAddr,
    version: u8,
    expect_user: Option<String>,
    expect_password: Option<String>,
) -> (SocketAddr, Task, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let recorded = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};let recorded=Arc::clone(&recorded);let user=expect_user.clone();let password=expect_password.clone();children.spawn(async move{let result:Result<()>=async{
            let mut credentials=json!({});if version==5{ensure!(socket.read_u8().await?==5,"bad SOCKS5 greeting");let count=socket.read_u8().await?;let mut methods=vec![0;count as usize];socket.read_exact(&mut methods).await?;let method=if user.is_some(){2}else{0};ensure!(methods.contains(&method),"missing authentication method");socket.write_all(&[5,method]).await?;
                if method==2{ensure!(socket.read_u8().await?==1,"bad authentication version");let mut name=vec![0;socket.read_u8().await? as usize];socket.read_exact(&mut name).await?;let mut pass=vec![0;socket.read_u8().await? as usize];socket.read_exact(&mut pass).await?;credentials=json!({"username":String::from_utf8(name)?,"password":String::from_utf8(pass)?});let okay=credentials["username"].as_str()==user.as_deref()&&credentials["password"].as_str()==password.as_deref();socket.write_all(&[1,if okay{0}else{1}]).await?;ensure!(okay,"wrong credentials");}
                let mut request=[0;4];socket.read_exact(&mut request).await?;ensure!(request[..3]==[5,1,0],"bad SOCKS5 CONNECT");let n=match request[3]{1=>4,4=>16,_=>anyhow::bail!("unsupported test address")};let mut addr=vec![0;n+2];socket.read_exact(&mut addr).await?;recorded.lock().unwrap().push(credentials);socket.write_all(&[5,0,0,1,0,0,0,0,0,0]).await?;
            }else{let mut request=[0;8];socket.read_exact(&mut request).await?;ensure!(request[..2]==[4,1],"bad SOCKS4 request");let mut userid=vec![];loop{let byte=socket.read_u8().await?;if byte==0{break}userid.push(byte);ensure!(userid.len()<256,"long user");}recorded.lock().unwrap().push(json!({"username":String::from_utf8(userid)?}));socket.write_all(&[0,90,0,0,0,0,0,0]).await?;}
            let mut upstream=TcpStream::connect(destination).await?;tokio::io::copy_bidirectional(&mut socket,&mut upstream).await?;Ok(())}.await;let _=result;});},Some(_)=children.join_next(),if !children.is_empty()=>{}}
        }
    });
    (address, Task(task), seen)
}
pub async fn connect_gateway(route: &proxy::Route, target: SocketAddr) -> Result<TcpStream> {
    let url = url::Url::parse(route.proxy_url.as_deref().context("gateway missing")?)?;
    let mut socket = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).await?;
    socket.write_all(&[5, 1, 2]).await?;
    let mut method = [0; 2];
    socket.read_exact(&mut method).await?;
    ensure!(method == [5, 2], "gateway did not demand authentication");
    let user = url.username().as_bytes();
    let pass = url.password().unwrap().as_bytes();
    let mut credentials = vec![1, user.len() as u8];
    credentials.extend(user);
    credentials.push(pass.len() as u8);
    credentials.extend(pass);
    socket.write_all(&credentials).await?;
    let mut response = [0; 2];
    socket.read_exact(&mut response).await?;
    ensure!(response == [1, 0], "gateway rejected credentials");
    let mut request = vec![5, 1, 0];
    match target.ip() {
        std::net::IpAddr::V4(ip) => {
            request.push(1);
            request.extend(ip.octets());
        }
        std::net::IpAddr::V6(ip) => {
            request.push(4);
            request.extend(ip.octets());
        }
    }
    request.extend(target.port().to_be_bytes());
    socket.write_all(&request).await?;
    let mut connected = [0; 10];
    socket.read_exact(&mut connected).await?;
    ensure!(
        connected == [5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        "gateway did not connect"
    );
    Ok(socket)
}
pub async fn mtproxy_server(secret_text: &str) -> (SocketAddr, Task) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let secret_text = secret_text.to_owned();
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {accepted=listener.accept()=>{let Ok((mut socket,_))=accepted else{break};let text=secret_text.clone();children.spawn(async move{let result:Result<()>=async{let secret=Secret::parse(&text)?;let mut head=[0;64];socket.read_exact(&mut head).await?;let (mut codec,_)=Obfuscation::from_client_header(head,&secret)?;loop{let mut length=[0;4];socket.read_exact(&mut length).await?;codec.decrypt(&mut length);let n=u32::from_le_bytes(length) as usize;ensure!(n<=mtproxy::MAX_FRAME,"frame too big");let mut payload=vec![0;n];socket.read_exact(&mut payload).await?;codec.decrypt(&mut payload);ensure!(payload.len()>=20&&payload[..8]==[0;8],"expected unencrypted probe");let declared=20+u32::from_le_bytes(payload[16..20].try_into().unwrap()) as usize;ensure!(declared<=payload.len()&&payload.len()-declared<=15,"invalid padded probe length");payload.truncate(declared);let response=reply(&payload,false)?;let mut frame=(response.len() as u32).to_le_bytes().to_vec();frame.extend(response);codec.encrypt(&mut frame);socket.write_all(&frame).await?;}}.await;let _=result;});},Some(_)=children.join_next(),if !children.is_empty()=>{}}
        }
    });
    (address, Task(task))
}
