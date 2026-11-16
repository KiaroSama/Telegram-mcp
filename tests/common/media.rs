#![allow(dead_code)]
use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, sync::Mutex};
use telegram_mcp::backend::Telegram;

pub struct MediaWire {
    pub peer: Value,
    pub responses: Mutex<VecDeque<(&'static str, Value)>>,
    pub calls: Mutex<Vec<(String, Value)>>,
    pub uploads: Mutex<Vec<std::path::PathBuf>>,
    pub downloads: Mutex<Vec<(Value, u64)>>,
    pub bytes: Vec<u8>,
}
impl MediaWire {
    pub fn new(responses: Vec<(&'static str, Value)>, bytes: Vec<u8>) -> Self {
        Self {
            peer: json!({"_":"inputPeerUser","user_id":"7","access_hash":"13"}),
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(vec![]),
            uploads: Mutex::new(vec![]),
            downloads: Mutex::new(vec![]),
            bytes,
        }
    }
    pub fn channel(mut self) -> Self {
        self.peer = json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"13"});
        self
    }
    pub fn chat(mut self) -> Self {
        self.peer = json!({"_":"inputPeerChat","chat_id":"7"});
        self
    }
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}
#[async_trait]
impl Telegram for MediaWire {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(method, &params)?;
        self.calls.lock().unwrap().push((method.into(), params));
        let (expected, response) = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected Telegram API call");
        ensure!(method == expected, "Expected {expected}, got {method}");
        Ok(response)
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        Ok(self.peer.clone())
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        ensure!(path.is_file(), "Upload source does not exist");
        self.uploads.lock().unwrap().push(path.to_owned());
        Ok(
            json!({"_":"inputFile","id":"42","parts":1,"name":path.file_name().unwrap().to_string_lossy(),"md5_checksum":format!("{:x}",md5::compute(std::fs::read(path)?))}),
        )
    }
    async fn download_memory(&self, location: Value, cap: u64) -> Result<Vec<u8>> {
        self.downloads.lock().unwrap().push((location, cap));
        ensure!(
            self.bytes.len() as u64 <= cap,
            "Stream exceeds transfer cap"
        );
        Ok(self.bytes.clone())
    }
    async fn download(&self, location: Value, path: &Path, cap: u64) -> Result<Value> {
        let bytes = self.download_memory(location, cap).await?;
        use std::io::Write;
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        Ok(json!({"path":path,"size":bytes.len()}))
    }
}
pub fn photo(id: i64) -> Value {
    json!({"_":"photo","id":id.to_string(),"access_hash":"9","file_reference":[1,2],"dc_id":2,"date":123,"sizes":[{"_":"photoSize","type":"m","w":10,"h":10,"size":9},{"_":"photoSize","type":"x","w":32,"h":16,"size":99}],"video_sizes":null})
}
pub fn document(id: i64, filename: &str, size: u64) -> Value {
    json!({"_":"document","id":id.to_string(),"access_hash":"9","file_reference":[1,2],"dc_id":2,"date":123,"mime_type":"application/octet-stream","size":size.to_string(),"thumbs":null,"video_thumbs":null,"attributes":[{"_":"documentAttributeFilename","file_name":filename}]})
}
pub fn media_message(media: Value) -> Value {
    json!({"_":"messages.messages","messages":[{"_":"message","id":8,"peer_id":{"_":"peerUser","user_id":"7"},"message":"","date":123,"media":media,"entities":[]}],"users":[],"chats":[]})
}
pub fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        32,
        16,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut out, image::ImageFormat::Png)
    .unwrap();
    out.into_inner()
}
pub fn wav() -> Vec<u8> {
    let samples = 16000u32;
    let bytes = samples * 2;
    let mut data = b"RIFF".to_vec();
    data.extend((36 + bytes).to_le_bytes());
    data.extend(b"WAVEfmt ");
    data.extend(16u32.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(16000u32.to_le_bytes());
    data.extend(32000u32.to_le_bytes());
    data.extend(2u16.to_le_bytes());
    data.extend(16u16.to_le_bytes());
    data.extend(b"data");
    data.extend(bytes.to_le_bytes());
    data.resize(data.len() + bytes as usize, 0);
    data
}
pub fn ffmpeg(arguments: &[&std::ffi::OsStr]) {
    let status = telegram_mcp::tools::messaging::media::child("ffmpeg")
        .as_std_mut()
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(arguments)
        .status()
        .expect("FFmpeg must be installed for native media CI");
    assert!(status.success(), "Could not create native media fixture");
}
