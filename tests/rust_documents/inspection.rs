use anyhow::{Result, bail};
use async_trait::async_trait;
use base64::Engine;
use image::{DynamicImage, ImageFormat};
use serde_json::{Value, json};
use std::{io::Cursor, sync::Mutex};
use telegram_mcp::{backend::Telegram, documents, tl::Schema};

struct Attachment {
    message: Value,
    bytes: Vec<u8>,
    calls: Mutex<Vec<String>>,
    downloaded: Mutex<Vec<(Value, u64)>>,
}
impl Attachment {
    fn text(name: &str, mime: &str, bytes: Vec<u8>) -> Self {
        Self {
            message: json!({"_":"message","id":4,"media":{"_":"messageMediaDocument","document":{"_":"document","id":"30","access_hash":"31","file_reference":[1,2,3],"mime_type":mime,"size":bytes.len(),"attributes":[{"_":"documentAttributeFilename","file_name":name}]}}}),
            bytes,
            calls: Mutex::new(Vec::new()),
            downloaded: Mutex::new(Vec::new()),
        }
    }
    async fn inspect(&self) -> Result<Value> {
        documents::inspect(self, &json!({"chat_id":7,"message_id":4})).await
    }
}
#[async_trait]
impl Telegram for Attachment {
    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        Schema::global().encode_call(method, &args)?;
        self.calls.lock().unwrap().push(method.into());
        if method == "channels.getMessages" {
            Ok(json!({"messages":[self.message]}))
        } else {
            bail!("Unexpected request")
        }
    }
    async fn resolve_peer(&self, _input: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}))
    }
    async fn download_memory(&self, location: Value, budget: u64) -> Result<Vec<u8>> {
        Schema::global().encode_object(&location)?;
        self.downloaded.lock().unwrap().push((location, budget));
        Ok(self.bytes.clone())
    }
}

#[tokio::test]
async fn utf8_document_is_read_in_memory_with_untrusted_metadata_and_no_seen_request() {
    let fixture = Attachment::text(
        "notes.txt",
        "text/plain",
        "سلام, Rust 🦀".as_bytes().to_vec(),
    );
    let result = fixture.inspect().await.unwrap();
    assert_eq!(result["text"], "سلام, Rust 🦀");
    assert_eq!(result["truncated"], false);
    assert_eq!(result["metadata"]["kind"], "text");
    assert_eq!(result["metadata"]["stored"], false);
    assert_eq!(result["metadata"]["untrusted"], true);
    assert_eq!(
        fixture.calls.lock().unwrap().as_slice(),
        ["channels.getMessages"]
    );
    assert_eq!(
        fixture.downloaded.lock().unwrap()[0].1,
        documents::MAX_DOCUMENT_BYTES
    );
}

#[test]
fn text_decoding_supports_utf8_and_both_utf16_byte_orders_without_loss() {
    let text = "Encoding سلام 🦀";
    let mut utf8 = vec![0xef, 0xbb, 0xbf];
    utf8.extend(text.as_bytes());
    assert_eq!(documents::decode_text(&utf8).unwrap(), text);
    for little in [false, true] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        assert_eq!(documents::decode_text(&bytes).unwrap(), text);
    }
}

#[test]
fn text_decoder_refuses_binary_invalid_utf8_partial_utf16_and_unpaired_surrogates() {
    for bytes in [
        vec![0, 1, 2],
        vec![0xff, 0x82],
        vec![0xff, 0xfe, 65],
        vec![0xff, 0xfe, 0, 0xd8],
    ] {
        assert!(documents::decode_text(&bytes).is_err());
    }
}

#[tokio::test]
async fn text_truncation_respects_utf8_boundaries_and_reports_the_cut() {
    let text = "界".repeat(documents::MAX_TEXT_BYTES / 3 + 2);
    let fixture = Attachment::text("large.md", "text/markdown", text.as_bytes().to_vec());
    let result = fixture.inspect().await.unwrap();
    let actual = result["text"].as_str().unwrap();
    assert_eq!(actual.len(), documents::MAX_TEXT_BYTES / 3 * 3);
    assert!(text.starts_with(actual));
    assert_eq!(result["truncated"], true);
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, height)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

#[tokio::test]
async fn image_documents_return_actual_bounded_png_pixels() {
    let fixture = Attachment::text("image.png", "image/png", png(1800, 900));
    let result = fixture.inspect().await.unwrap();
    assert_eq!(result["metadata"]["kind"], "image");
    assert_eq!(result["metadata"]["stored"], false);
    let content = &result["content"][0];
    assert_eq!(content["type"], "image");
    assert_eq!(content["mimeType"], "image/png");
    let decoded = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(content["data"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1568, 784));
}

#[tokio::test]
async fn photo_messages_are_inspected_without_a_document_filename() {
    let mut fixture = Attachment::text("unused", "", png(32, 16));
    fixture.message["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"30","access_hash":"31","file_reference":[1,2,3],"sizes":[{"_":"photoSize","type":"x","w":32,"h":16,"size":fixture.bytes.len()}]}});
    let result = fixture.inspect().await.unwrap();
    assert_eq!(result["metadata"]["kind"], "image");
    assert_eq!(
        fixture.downloaded.lock().unwrap()[0].0["_"],
        "inputPhotoFileLocation"
    );
}

#[tokio::test]
async fn actual_image_magic_takes_precedence_over_a_misleading_pdf_filename() {
    let fixture = Attachment::text("misnamed.pdf", "application/pdf", png(8, 8));
    assert_eq!(
        fixture.inspect().await.unwrap()["metadata"]["kind"],
        "image"
    );
}

#[tokio::test]
async fn malformed_images_and_pdf_headers_are_refused_without_parser_fallback() {
    for (name, mime) in [
        ("image.png", "image/png"),
        ("document.pdf", "application/pdf"),
    ] {
        let fixture = Attachment::text(name, mime, b"not the declared content".to_vec());
        assert!(fixture.inspect().await.is_err());
    }
}

#[tokio::test]
async fn unsupported_attachments_and_oversized_declared_files_are_not_downloaded() {
    let fixture = Attachment::text(
        "program.exe",
        "application/octet-stream",
        b"binary".to_vec(),
    );
    assert!(fixture.inspect().await.is_err());
    assert!(fixture.downloaded.lock().unwrap().is_empty());
    let mut fixture = Attachment::text("large.txt", "text/plain", b"small".to_vec());
    fixture.message["media"]["document"]["size"] = json!(documents::MAX_DOCUMENT_BYTES + 1);
    assert!(fixture.inspect().await.is_err());
    assert!(fixture.downloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn actual_download_size_is_rechecked_independently_of_declared_size() {
    let mut fixture = Attachment::text(
        "large.txt",
        "text/plain",
        vec![b'x'; documents::MAX_DOCUMENT_BYTES as usize + 1],
    );
    fixture.message["media"]["document"]["size"] = json!(1);
    assert!(fixture.inspect().await.is_err());
    assert_eq!(fixture.downloaded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn empty_download_and_missing_media_are_not_reported_as_empty_documents() {
    let fixture = Attachment::text("empty.txt", "text/plain", Vec::new());
    assert!(fixture.inspect().await.is_err());
    let mut fixture = Attachment::text("unused.txt", "text/plain", vec![1]);
    fixture.message["media"] = Value::Null;
    assert!(fixture.inspect().await.is_err());
    assert!(fixture.downloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn topic_mismatch_prevents_download_and_matching_topic_keeps_scope() {
    let mut fixture = Attachment::text("topic.txt", "text/plain", b"scoped document".to_vec());
    fixture.message["reply_to"] = json!({"forum_topic":true,"reply_to_top_id":8});
    assert!(
        documents::inspect(&fixture, &json!({"chat_id":7,"message_id":4,"topic_id":9}))
            .await
            .is_err()
    );
    assert!(fixture.downloaded.lock().unwrap().is_empty());
    let result = documents::inspect(&fixture, &json!({"chat_id":7,"message_id":4,"topic_id":8}))
        .await
        .unwrap();
    assert_eq!(result["text"], "scoped document");
}

#[tokio::test]
async fn invalid_topic_ids_are_rejected_before_any_telegram_request() {
    for topic in [json!(0), json!(-1), json!(2147483648i64), json!("4")] {
        let fixture = Attachment::text("topic.txt", "text/plain", b"scoped".to_vec());
        assert!(
            documents::inspect(
                &fixture,
                &json!({"chat_id":7,"message_id":4,"topic_id":topic})
            )
            .await
            .is_err()
        );
        assert!(fixture.calls.lock().unwrap().is_empty());
        assert!(fixture.downloaded.lock().unwrap().is_empty());
    }
}
