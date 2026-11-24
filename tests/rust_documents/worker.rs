use base64::Engine;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

pub(super) fn pdf(text: Option<&str>) -> Vec<u8> {
    let stream = text
        .map(|text| format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET"))
        .unwrap_or_default();
    let objects=[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{}\nendstream",stream.len(),stream),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
    ];
    pdf_objects(&objects)
}

pub(super) fn pdf_objects(objects: &[String]) -> Vec<u8> {
    let mut output = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = output.len();
    output.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.iter().skip(1) {
        output.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    output.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    output
}

fn request(bytes: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({"data":base64::engine::general_purpose::STANDARD.encode(bytes)}))
        .unwrap()
}

async fn run(input: Vec<u8>) -> Value {
    let temporary = tempfile::tempdir().unwrap();
    // A hidden worker must bypass configuration and never create local state.
    std::fs::write(temporary.path().join(".env"), "BROKEN='unclosed\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
    command
        .arg("__document-worker")
        .current_dir(temporary.path())
        .env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "LLVM_PROFILE_FILE"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env(
            "TELEGRAM_API_HASH",
            "synthetic-credential-must-not-be-printed",
        )
        .env(
            "TELEGRAM_SESSION_STRING",
            "synthetic-session-must-not-be-printed",
        );
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let operation = async {
        let mut stdin = child.stdin.take().unwrap();
        let written = stdin.write_all(&input).await;
        drop(stdin);
        if let Err(error) = written {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        child.wait_with_output().await.unwrap()
    };
    let output = tokio::time::timeout(Duration::from_secs(55), operation)
        .await
        .expect("Bounded PDF worker did not terminate");
    assert!(
        output.status.success(),
        "Worker failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "Worker emitted stderr output");
    assert!(output.stdout.len() <= telegram_mcp::documents::MAX_TEXT_BYTES * 6 + 4096);
    let result: Value = serde_json::from_slice(&output.stdout)
        .expect("Worker stdout must be exactly one JSON value");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("synthetic-credential-must-not-be-printed"));
    assert!(!text.contains("synthetic-session-must-not-be-printed"));
    let names = std::fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [std::ffi::OsString::from(".env")]);
    result
}

#[tokio::test]
async fn native_worker_extracts_real_pdf_text_without_external_executables_or_state_files() {
    let result = run(request(&pdf(Some("Native Rust PDF extraction canary")))).await;
    assert_eq!(result["ok"], true);
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("Native Rust PDF extraction canary")
    );
    assert_eq!(result["truncated"], false);
    assert!(result["reason"].is_null());
}

#[tokio::test]
async fn native_worker_reports_a_pdf_without_a_text_layer_instead_of_inventing_ocr() {
    let result = run(request(&pdf(None))).await;
    assert_eq!(result["ok"], true);
    assert!(result["text"].as_str().unwrap().trim().is_empty());
    assert_eq!(result["reason"], "no_text_layer");
}

#[tokio::test]
async fn malformed_pdf_and_worker_requests_fail_with_generic_payload_free_errors() {
    for input in [
        request(b"%PDF-1.7\nprivate-document-fragment-with-no-xref"),
        b"{invalid-private-json".to_vec(),
        serde_json::to_vec(&json!({"data":"not-base64-private-fragment!"})).unwrap(),
        request(b"ordinary text is not a PDF"),
    ] {
        let result = run(input).await;
        assert_eq!(result["ok"], false);
        assert_eq!(
            result["error"],
            "PDF text extraction failed; malformed, encrypted or unsupported PDF"
        );
        assert!(result.get("text").is_none());
    }
}

#[tokio::test]
async fn oversized_pdf_input_is_refused_within_the_worker_document_budget() {
    let mut bytes = vec![b' '; telegram_mcp::documents::MAX_DOCUMENT_BYTES as usize + 1];
    bytes[..8].copy_from_slice(b"%PDF-1.4");
    let result = run(request(&bytes)).await;
    assert_eq!(result["ok"], false);
}

#[tokio::test]
async fn oversized_raw_worker_request_stops_reading_at_its_own_bound() {
    let result = run(vec![b' '; 36 * 1024 * 1024 + 1024]).await;
    assert_eq!(result["ok"], false);
    assert!(result.get("text").is_none());
}

pub(super) fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

pub(super) fn unicode_pdf() -> Vec<u8> {
    let cmap = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /SyntheticUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n6 beginbfchar\n<0001> <0633>\n<0002> <0644>\n<0003> <0627>\n<0004> <0645>\n<0005> <D83EDD80>\n<0006> <00660069>\nendbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
        stream("BT /F1 12 Tf 72 720 Td <000100020003000400050006> Tj ET"),
        "<< /Type /Font /Subtype /Type0 /BaseFont /SyntheticUnicode /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>".into(),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /SyntheticUnicode /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>".into(),
        stream(cmap),
    ];
    pdf_objects(&objects)
}

#[tokio::test]
async fn native_pdf_worker_decodes_real_tounicode_cmap_including_surrogate_pairs() {
    let result = run(request(&unicode_pdf())).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["text"].as_str().unwrap().trim(), "سلام🦀fi");
}

#[tokio::test]
async fn native_pdf_worker_preserves_logical_page_order_when_object_numbers_differ() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Count 2 /Kids [6 0 R 3 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
        stream("BT /F1 12 Tf 72 720 Td (Second logical page) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>".into(),
        stream("BT /F1 12 Tf 72 720 Td (First logical page) Tj ET"),
    ];
    let result = run(request(&pdf_objects(&objects))).await;
    assert_eq!(result["ok"], true);
    let text = result["text"].as_str().unwrap();
    assert!(text.find("First logical page").unwrap() < text.find("Second logical page").unwrap());
}

#[tokio::test]
async fn unsupported_pdf_form_text_is_refused_instead_of_returned_as_a_missing_text_layer() {
    let form = "BT /F1 12 Tf 72 720 Td (Text inside a form) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>".into(),
        stream("/Fm1 Do"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n{form}\nendstream", form.len()),
    ];
    let result = run(request(&pdf_objects(&objects))).await;
    assert_eq!(result["ok"], false);
    assert!(result.get("text").is_none());
    assert!(result.get("reason").is_none());
}
