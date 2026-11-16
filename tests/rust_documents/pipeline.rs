//! Exercise the bounded parent pipe with actual subprocesses and mock Telegram.
use super::{
    inspection::Attachment,
    worker::{pdf, pdf_objects, stream, unicode_pdf},
};
use serde_json::json;
use std::{path::PathBuf, sync::OnceLock};
use telegram_mcp::documents::{self, PdfWorker};

fn native() -> PdfWorker {
    PdfWorker::new(env!("CARGO_BIN_EXE_telegram-mcp"))
}
async fn inspect(fixture: &Attachment) -> anyhow::Result<serde_json::Value> {
    documents::inspect_with_worker(fixture, &json!({"chat_id":7,"message_id":4}), &native()).await
}

#[tokio::test]
async fn pdf_inspection_uses_the_real_worker_pipe_and_keeps_telegram_data_in_memory() {
    let fixture = Attachment::text(
        "report.pdf",
        "application/pdf",
        pdf(Some("Complete parent pipe canary")),
    );
    let result = inspect(&fixture).await.unwrap();
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("Complete parent pipe canary")
    );
    assert_eq!(result["metadata"]["filename"], "report.pdf");
    assert_eq!(result["metadata"]["kind"], "pdf");
    assert_eq!(result["metadata"]["stored"], false);
    assert_eq!(result["metadata"]["untrusted"], true);
    assert_eq!(result["truncated"], false);
    assert!(result["reason"].is_null());
    assert_eq!(
        fixture.calls.lock().unwrap().as_slice(),
        ["channels.getMessages"]
    );
    assert_eq!(fixture.downloaded.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.downloaded.lock().unwrap()[0].1,
        documents::MAX_DOCUMENT_BYTES
    );
}

#[tokio::test]
async fn unicode_cmap_survives_download_request_serialization_child_and_parent_decoding() {
    let fixture = Attachment::text("unicode.PDF", "application/octet-stream", unicode_pdf());
    let result = inspect(&fixture).await.unwrap();
    assert_eq!(result["text"].as_str().unwrap().trim(), "سلام🦀fi");
    assert_eq!(result["metadata"]["kind"], "pdf");
}

#[tokio::test]
async fn scanned_or_empty_pdf_reason_survives_the_real_parent_pipe() {
    let fixture = Attachment::text("scan.pdf", "application/pdf", pdf(None));
    let result = inspect(&fixture).await.unwrap();
    assert!(result["text"].as_str().unwrap().trim().is_empty());
    assert_eq!(result["reason"], "no_text_layer");
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn malformed_pdf_is_a_generic_error_and_does_not_poison_the_next_worker() {
    let fixture = Attachment::text(
        "private-name.pdf",
        "application/pdf",
        b"%PDF-1.7\nprivate-byte-canary-with-no-xref".to_vec(),
    );
    let error = inspect(&fixture).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("PDF text extraction failed"));
    assert!(!message.contains("private-byte-canary"));
    assert!(!message.contains("private-name.pdf"));
    let fixture = Attachment::text(
        "valid.pdf",
        "application/pdf",
        pdf(Some("Worker permit released")),
    );
    assert!(
        inspect(&fixture).await.unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Worker permit released")
    );
}

#[tokio::test]
async fn pdf_text_cap_is_enforced_after_real_extraction_and_pipe_decoding() {
    let text = "x".repeat(documents::MAX_TEXT_BYTES + 64);
    let fixture = Attachment::text("long.pdf", "application/pdf", pdf(Some(&text)));
    let result = inspect(&fixture).await.unwrap();
    assert_eq!(result["truncated"], true);
    assert_eq!(
        result["text"].as_str().unwrap().len(),
        documents::MAX_TEXT_BYTES
    );
    assert!(text.starts_with(result["text"].as_str().unwrap()));
}

#[tokio::test]
async fn compressed_pdf_content_cannot_exceed_the_per_stream_budget() {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(&vec![b' '; documents::MAX_DOCUMENT_BYTES as usize + 1])
        .unwrap();
    let compressed = encoder.finish().unwrap();
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();
    let content = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {"Filter"=>"FlateDecode"},
        compressed,
    ));
    let page=document.add_object(lopdf::dictionary!{"Type"=>"Page","Parent"=>pages_id,"Contents"=>content,"MediaBox"=>vec![0.into(),0.into(),612.into(),792.into()]});
    document.objects.insert(
        pages_id,
        lopdf::dictionary! {"Type"=>"Pages","Kids"=>vec![page.into()],"Count"=>1}.into(),
    );
    let root = document.add_object(lopdf::dictionary! {"Type"=>"Catalog","Pages"=>pages_id});
    document.trailer.set("Root", root);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    assert!(bytes.len() < documents::MAX_DOCUMENT_BYTES as usize);
    let fixture = Attachment::text("compressed.pdf", "application/pdf", bytes);
    assert!(inspect(&fixture).await.is_err());
}

#[tokio::test]
async fn page_numbers_remain_logical_order_through_parent_transport() {
    let objects=["<< /Type /Catalog /Pages 2 0 R >>".into(),"<< /Type /Pages /Count 2 /Kids [6 0 R 3 0 R] >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),stream("BT /F1 12 Tf (Second page) Tj ET"),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".into(),stream("BT /F1 12 Tf (First page) Tj ET")];
    let fixture = Attachment::text("pages.pdf", "application/pdf", pdf_objects(&objects));
    let result = inspect(&fixture).await.unwrap();
    let text = result["text"].as_str().unwrap();
    assert!(text.find("First page").unwrap() < text.find("Second page").unwrap());
}

struct PeerFixture {
    directory: tempfile::TempDir,
    executable: PathBuf,
}
fn fixture_worker(mode: &str) -> PdfWorker {
    static FIXTURE: OnceLock<PeerFixture> = OnceLock::new();
    let fixture = FIXTURE.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("worker.rs");
        std::fs::write(&source, include_str!("../fixtures/document_worker.rs")).unwrap();
        let executable = directory.path().join(format!(
            "document-worker-fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = std::process::Command::new(compiler)
            .args(["--edition=2024", "--crate-name=document_worker_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not compile the Rust protocol fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        PeerFixture {
            directory,
            executable,
        }
    });
    let executable = fixture
        .directory
        .path()
        .join(format!("worker-{mode}{}", std::env::consts::EXE_SUFFIX));
    if !executable.exists() {
        std::fs::copy(&fixture.executable, &executable).unwrap();
    }
    PdfWorker::new(executable)
}

#[tokio::test]
async fn parent_worker_environment_contains_only_loader_and_profile_variables() {
    let worker = fixture_worker("environment");
    let result = worker.extract(b"synthetic document bytes").await.unwrap();
    let keys = result["text"]
        .as_str()
        .unwrap()
        .split(',')
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    assert!(std::env::var_os("PATH").is_some());
    let same_name = |key: &str, allowed: &str| {
        if cfg!(windows) {
            key.eq_ignore_ascii_case(allowed)
        } else {
            key == allowed
        }
    };
    let unexpected = keys
        .iter()
        .filter(|key| {
            !["SystemRoot", "WINDIR", "TEMP", "TMP", "LLVM_PROFILE_FILE"]
                .iter()
                .any(|allowed| same_name(key, allowed))
        })
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "Unexpected worker environment KEY NAMES: {unexpected:?}"
    );
    assert!(!keys.iter().any(|key| same_name(key, "PATH")));
    assert!(!keys.iter().any(|key| same_name(key, "HOME")));
    let forbidden = keys
        .iter()
        .filter(|key| {
            let key = if cfg!(windows) {
                key.to_ascii_uppercase()
            } else {
                key.to_string()
            };
            key.starts_with("TELEGRAM_") || key.starts_with("CARGO_")
        })
        .collect::<Vec<_>>();
    assert!(
        forbidden.is_empty(),
        "Inherited worker environment KEY NAMES: {forbidden:?}"
    );
}

#[tokio::test]
async fn malformed_or_nonzero_worker_responses_do_not_escape_as_document_content() {
    for mode in [
        "failure",
        "malformed",
        "invalid-text",
        "invalid-truncation",
        "invalid-reason",
    ] {
        let error = fixture_worker(mode)
            .extract(b"synthetic document bytes")
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("worker-private-canary"));
    }
}

#[tokio::test]
async fn oversized_worker_json_or_text_is_rejected_and_the_process_is_reaped() {
    for mode in ["oversized", "oversized-text"] {
        assert!(
            fixture_worker(mode)
                .extract(b"synthetic document bytes")
                .await
                .is_err()
        );
    }
    assert!(
        native()
            .extract(&pdf(Some("Still usable after worker output rejection")))
            .await
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Still usable")
    );
}

#[tokio::test]
async fn early_worker_exit_during_parent_input_is_an_error_and_releases_the_permit() {
    assert!(
        fixture_worker("early-exit")
            .extract(&vec![b'x'; 4 * 1024 * 1024])
            .await
            .is_err()
    );
    assert!(
        native()
            .extract(&pdf(Some("Permit after broken pipe")))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn oversized_input_is_rejected_before_attempting_to_start_an_executable() {
    let temporary = tempfile::tempdir().unwrap();
    let worker = PdfWorker::new(temporary.path().join("does-not-exist"));
    let error = worker
        .extract(&vec![0; documents::MAX_DOCUMENT_BYTES as usize + 1])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("byte budget"));
    let error = worker
        .extract(&pdf(Some("cannot start")))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Could not start bounded document worker")
    );
}
