use super::*;
use std::io::{Read, Write};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const MAX_REQUEST: usize = 36 * 1024 * 1024;
// A JSON string can require six bytes per source byte (e.g. U+0000).
const MAX_RESPONSE: usize = MAX_TEXT_BYTES * 6 + 4096;
static WORKERS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
#[cfg(any(target_os = "macos", test))]
mod memory;

pub fn worker_entry() -> Result<()> {
    #[cfg(windows)]
    let _limits = limits()?;
    #[cfg(not(any(windows, target_os = "macos")))]
    limits()?;
    #[cfg(target_os = "macos")]
    let limits = limits()?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST as u64 + 1)
        .read_to_end(&mut input)?;
    let result = (|| -> Result<Value> {
        ensure!(
            input.len() <= MAX_REQUEST,
            "Document worker request exceeds its byte budget"
        );
        let request: Value =
            serde_json::from_slice(&input).context("Invalid document worker request")?;
        let encoded = request["data"]
            .as_str()
            .context("Document worker request has no data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("Invalid document worker encoding")?;
        ensure!(
            bytes.len() as u64 <= MAX_DOCUMENT_BYTES,
            "Document exceeds its byte budget"
        );
        ensure!(
            bytes.starts_with(b"%PDF-"),
            "Document worker accepts PDF data only"
        );
        let text = extract_pdf_text(&bytes)?;
        let (text, truncated) = bounded_text(text);
        let empty = text.trim().is_empty();
        Ok(
            json!({"ok":true,"text":text,"truncated":truncated,"reason":if empty{Some("no_text_layer")}else{None}}),
        )
    })();
    let answer=result.unwrap_or_else(|_|json!({"ok":false,"error":"PDF text extraction failed; malformed, encrypted or unsupported PDF"}));
    let output = serde_json::to_vec(&answer)?;
    ensure!(
        output.len() <= MAX_RESPONSE,
        "Document worker response exceeds its byte budget"
    );
    // Check the lifetime high-water mark after all parsing/serialization and
    // before releasing any result. The supervisor starts before stdin is read.
    #[cfg(target_os = "macos")]
    limits
        .finish()
        .context("Document worker exceeded its memory budget or accounting failed")?;
    std::io::stdout().write_all(&output)?;
    std::io::stdout().flush()?;
    Ok(())
}

fn extract_pdf_text(bytes: &[u8]) -> Result<String> {
    let document = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            strict: true,
            max_decompressed_size: Some(MAX_DOCUMENT_BYTES as usize),
            ..Default::default()
        },
    )
    .context("PDF could not be parsed within its stream limits")?;
    ensure!(!document.is_encrypted(), "PDF requires a password");
    let pages = document.get_pages();
    ensure!(
        !pages.is_empty() && pages.len() <= 10_000,
        "PDF has no readable pages or exceeds its page budget"
    );
    for page in pages.values() {
        reject_unsupported_forms(&document, *page)?;
    }
    // get_pages is a BTreeMap, so the official extractor receives logical page
    // order rather than PDF object-number order. Font embedding is unnecessary.
    document
        .extract_text_with_limit(
            &pages.keys().copied().collect::<Vec<_>>(),
            MAX_DOCUMENT_BYTES as usize,
        )
        .context("PDF text has an unsupported encoding or layout")
}

fn reject_unsupported_forms(document: &lopdf::Document, page: lopdf::ObjectId) -> Result<()> {
    let data = document.get_page_content_with_limit(page, MAX_DOCUMENT_BYTES as usize)?;
    let (direct, referenced) = document.get_page_resources(page)?;
    let mut resources: Vec<_> = direct.into_iter().collect();
    for reference in referenced {
        resources.push(document.get_dictionary(reference)?);
    }
    let fonts = document.get_page_fonts(page)?;
    let mut has_font = false;
    for operation in lopdf::content::Content::decode_iter(&data) {
        let operation = operation?;
        if operation.operator == "Tf" {
            let name = operation
                .operands
                .first()
                .context("PDF text has no font name")?
                .as_name()?;
            ensure!(
                fonts.contains_key(name),
                "PDF text refers to a missing font"
            );
            has_font = true;
        }
        if matches!(operation.operator.as_str(), "Tj" | "TJ" | "'" | "\"") {
            ensure!(has_font, "PDF text has no decodable font");
        }
        if operation.operator != "Do" {
            continue;
        }
        let name = operation
            .operands
            .first()
            .context("PDF XObject has no resource name")?
            .as_name()?;
        let object = resources
            .iter()
            .find_map(|resources| {
                document
                    .get_dict_in_dict(resources, b"XObject")
                    .ok()
                    .and_then(|objects| objects.get(name).ok())
            })
            .context("PDF XObject resource is missing")?;
        let stream = document.dereference(object)?.1.as_stream()?;
        // lopdf's page extractor does not descend Form XObjects. Refuse an
        // unsupported form instead of presenting silently missing text as a
        // document without a text layer (or presenting partial text as complete).
        ensure!(
            stream.dict.get(b"Subtype")?.as_name()? != b"Form",
            "PDF Form XObject text layout is unsupported"
        );
    }
    Ok(())
}
pub(super) async fn extract(executable: &std::path::Path, bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() as u64 <= MAX_DOCUMENT_BYTES,
        "Document exceeds its byte budget"
    );
    let semaphore = WORKERS.get_or_init(|| tokio::sync::Semaphore::new(2));
    let _permit = tokio::time::timeout(std::time::Duration::from_secs(5), semaphore.acquire())
        .await
        .context("Document workers are busy; retry later")?
        .context("Document workers are unavailable")?;
    let request = serde_json::to_vec(
        &json!({"data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
    )?;
    ensure!(
        request.len() <= MAX_REQUEST,
        "Document worker request exceeds its byte budget"
    );
    let mut command = tokio::process::Command::new(executable);
    command.env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "LLVM_PROFILE_FILE"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .arg("__document-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("Could not start bounded document worker")?;
    let mut stdin = child
        .stdin
        .take()
        .context("Document worker has no input pipe")?;
    let stdout = child
        .stdout
        .take()
        .context("Document worker has no output pipe")?;
    let run = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stdin.write_all(&request),
        )
        .await
        .context("Document worker input timed out")??;
        drop(stdin);
        let mut output = Vec::new();
        stdout
            .take(MAX_RESPONSE as u64 + 1)
            .read_to_end(&mut output)
            .await?;
        ensure!(
            output.len() <= MAX_RESPONSE,
            "Document worker output exceeds its byte budget"
        );
        ensure!(
            child.wait().await?.success(),
            "Document worker stopped before extraction completed"
        );
        let result: Value = serde_json::from_slice(&output)
            .context("Document worker returned an invalid result")?;
        ensure!(
            result["ok"] == true,
            "PDF text extraction failed; malformed, encrypted or unsupported PDF"
        );
        let text = result["text"]
            .as_str()
            .context("Document worker returned invalid text")?;
        ensure!(
            text.len() <= MAX_TEXT_BYTES,
            "Document worker text exceeds its byte budget"
        );
        ensure!(
            result["truncated"].is_boolean(),
            "Document worker returned an invalid truncation flag"
        );
        ensure!(
            result["reason"].is_null() || result["reason"] == "no_text_layer",
            "Document worker returned an invalid text-layer reason"
        );
        Ok::<_, anyhow::Error>(result)
    };
    let result = match tokio::time::timeout(std::time::Duration::from_secs(40), run).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("Document worker exceeded its time budget")),
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
#[cfg(all(unix, not(target_os = "macos")))]
fn limits() -> Result<()> {
    let memory = libc::rlimit {
        rlim_cur: 512 * 1024 * 1024,
        rlim_max: 512 * 1024 * 1024,
    };
    // SAFETY: called in the dedicated synchronous parser process before threads
    // or network clients exist; the pointer refers to an initialized limit.
    unsafe {
        ensure!(
            libc::setrlimit(libc::RLIMIT_AS, &memory) == 0,
            "Cannot bound document worker address space"
        );
    }
    cpu_limit()
}
#[cfg(target_os = "macos")]
fn limits() -> Result<memory::MemoryGuard> {
    cpu_limit()?;
    // Darwin rejects RLIMIT_AS below the address space mapped at launch (which
    // includes its shared cache). Measure physical memory plus its lifetime
    // peak instead; this is a sampled limit, not an address-space reservation.
    memory::MemoryGuard::start(
        512 * 1024 * 1024,
        std::time::Duration::from_millis(20),
        memory::physical_footprint,
    )
    .context("Cannot supervise document worker memory")
}
#[cfg(unix)]
fn cpu_limit() -> Result<()> {
    let cpu = libc::rlimit {
        rlim_cur: 20,
        rlim_max: 20,
    };
    // SAFETY: the dedicated parser process applies an initialized CPU limit
    // before it creates its watchdog thread or reads any untrusted input.
    ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } == 0,
        "Cannot bound document worker CPU time"
    );
    Ok(())
}
#[cfg(windows)]
fn limits() -> Result<WindowsJob> {
    use windows_sys::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};
    // SAFETY: initialized POD structure and an owned kernel handle; no borrowed
    // memory survives these calls. The guard keeps limits active until exit.
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        ensure!(
            !handle.is_null(),
            "Cannot create bounded document worker job"
        );
        let job = WindowsJob(handle);
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        information.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_PROCESS_TIME;
        information.BasicLimitInformation.PerProcessUserTimeLimit = 20 * 10_000_000;
        information.ProcessMemoryLimit = 512 * 1024 * 1024;
        ensure!(
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &information as *const _ as *const _,
                std::mem::size_of_val(&information) as u32
            ) != 0,
            "Cannot limit document worker memory"
        );
        ensure!(
            AssignProcessToJobObject(handle, GetCurrentProcess()) != 0,
            "Cannot attach document worker limits"
        );
        Ok(job)
    }
}
#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(not(any(unix, windows)))]
fn limits() -> Result<()> {
    anyhow::bail!("Bounded document processing is unsupported on this operating system")
}
