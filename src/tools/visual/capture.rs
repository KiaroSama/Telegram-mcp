//! Captures run in a bounded child: PrintWindow cannot be cancelled in a thread.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const MAX_CAPTURE_SIDE: i64 = 16_384;
pub const MAX_CAPTURE_PIXELS: i64 = 32_000_000;
pub const MAX_CAPTURE_RESPONSE_BYTES: u64 = 48 * 1024 * 1024;
static WORKERS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

#[path = "containment.rs"]
pub(crate) mod containment;

pub fn check_allocation(width: i64, height: i64) -> Result<()> {
    if width <= 0 || height <= 0 {
        bail!("Telegram window has no drawable area");
    }
    if width > MAX_CAPTURE_SIDE
        || height > MAX_CAPTURE_SIDE
        || width
            .checked_mul(height)
            .is_none_or(|pixels| pixels > MAX_CAPTURE_PIXELS)
    {
        bail!("Telegram window dimensions exceed the capture allocation limit");
    }
    Ok(())
}

pub fn normalize(name: &str, args: &Value) -> Result<Value> {
    let mut request = args.clone();
    if !request.is_object() {
        bail!("Capture arguments must be an object");
    }
    let method = args["method"].as_str().unwrap_or("window");
    if !["window", "screen"].contains(&method) {
        bail!("Unknown capture method; expected window or screen");
    }
    let format = args["image_format"]
        .as_str()
        .unwrap_or("png")
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if !["png", "jpeg", "jpg", "webp"].contains(&format.as_str()) {
        bail!("Unsupported image_format; expected png, jpeg, jpg or webp");
    }
    let process = args["process_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("TELEGRAM_DESKTOP_PROCESS").ok())
        .unwrap_or_else(|| "Telegram.exe".into());
    if process.len() > 260 || process.contains(['/', '\\']) || process.chars().any(char::is_control)
    {
        bail!("process_name must be an executable file name");
    }
    request["tool"] = name.into();
    request["method"] = method.into();
    request["image_format"] = format.into();
    request["process_name"] = process.into();
    request["count"] = if name == "get_telegram_frames" {
        crate::args::integer(args, "count", 4)?.clamp(1, 8)
    } else {
        1
    }
    .into();
    request["interval_ms"] = crate::args::integer(args, "interval_ms", 400)?
        .clamp(50, 3000)
        .into();
    let default = if name == "get_telegram_frames" {
        900
    } else {
        1568
    };
    request["max_dimension"] =
        super::images::bounded_dimension(crate::args::integer(args, "max_dimension", default)?)
            .into();
    if name == "get_telegram_region" {
        let left = crate::args::integer(args, "left", -1)?;
        let top = crate::args::integer(args, "top", -1)?;
        let right = crate::args::integer(args, "right", -1)?;
        let bottom = crate::args::integer(args, "bottom", -1)?;
        if left < 0 || top < 0 || right <= left || bottom <= top {
            bail!("Capture region must be a nonempty rectangle with nonnegative coordinates");
        }
        check_allocation(right - left, bottom - top)?;
        for (key, value) in [
            ("left", left),
            ("top", top),
            ("right", right),
            ("bottom", bottom),
        ] {
            request[key] = value.into();
        }
    }
    Ok(request)
}

pub async fn request(name: &str, args: &Value) -> Result<Value> {
    let request = normalize(name, args)?;
    #[cfg(not(windows))]
    {
        let _ = request;
        bail!(
            "Telegram Desktop window capture is available only on Windows; use structured message or media inspection on this platform"
        );
    }
    #[cfg(windows)]
    {
        run_child(request).await
    }
}

/// Called before the command-line parser and tracing setup in the executable.
/// This private child command receives no Telegram sessions or credentials.
pub async fn worker_entry() -> Result<bool> {
    if std::env::args().nth(1).as_deref() != Some("__capture-worker") {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > 64 * 1024 {
        bail!("Capture request exceeds the input limit");
    }
    let args: Value =
        serde_json::from_slice(&bytes).context("Capture request is not valid JSON")?;
    let result = if args["tool"].as_str() == Some("__decode_media") {
        super::frames::worker(&args)
    } else {
        #[cfg(windows)]
        {
            super::win32::run(&args)
        }
        #[cfg(not(windows))]
        {
            Err(anyhow::anyhow!("Window capture requires Windows"))
        }
    };
    let response = match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    let data = serde_json::to_vec(&response)?;
    if data.len() as u64 > MAX_CAPTURE_RESPONSE_BYTES {
        bail!("Capture response exceeds the byte limit");
    }
    tokio::io::stdout().write_all(&data).await?;
    Ok(true)
}

pub(crate) async fn run_child(request: Value) -> Result<Value> {
    let semaphore = WORKERS.get_or_init(|| tokio::sync::Semaphore::new(2));
    let _permit = tokio::time::timeout(std::time::Duration::from_secs(5), semaphore.acquire())
        .await
        .context("Capture workers are busy; retry later")?
        .context("Capture workers are unavailable")?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command.arg("__capture-worker").env_clear();
    // Native image decoders do not need access to Telegram authorization
    // environment variables. Only OS process-loading variables are inherited.
    for key in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "LANG",
        "LC_ALL",
        // Instrumented CI children need their own %p profile output. This is
        // measurement configuration, never an account/API credential.
        "LLVM_PROFILE_FILE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(value) =
        std::env::var_os("TELEGRAM_RLOTTIE_LIBRARY").or_else(|| std::env::var_os("RLOTTIE_LIBRARY"))
    {
        command.env("TELEGRAM_RLOTTIE_LIBRARY", value);
    }
    let (mut child, _tree) = containment::spawn(
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true),
    )
    .context("Capture worker could not be started")?;
    let input = serde_json::to_vec(&request)?;
    let mut stdin = child
        .stdin
        .take()
        .context("Capture worker has no input pipe")?;
    stdin.write_all(&input).await?;
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .context("Capture worker has no output pipe")?;
    let read = async {
        let mut output = Vec::new();
        stdout
            .take(MAX_CAPTURE_RESPONSE_BYTES + 1)
            .read_to_end(&mut output)
            .await?;
        if output.len() as u64 > MAX_CAPTURE_RESPONSE_BYTES {
            bail!("Capture response exceeds the byte limit");
        }
        let status = child.wait().await?;
        if !status.success() {
            bail!("Capture worker exited before returning a complete result");
        }
        let response: Value =
            serde_json::from_slice(&output).context("Capture worker returned invalid JSON")?;
        if response["ok"] != true {
            bail!("{}", response["error"].as_str().unwrap_or("Capture failed"));
        }
        Ok(response["result"].clone())
    };
    let count = request["count"].as_u64().unwrap_or(1).clamp(1, 8);
    let spacing = request["interval_ms"].as_u64().unwrap_or(400).min(3000);
    let deadline = std::time::Duration::from_millis(15_000 + 5_000 * count + (count - 1) * spacing);
    match tokio::time::timeout(deadline, read).await {
        Ok(result) => {
            if result.is_err() {
                let _ = child.kill().await;
            }
            result
        }
        Err(_) => {
            let _ = child.kill().await;
            bail!(
                "Telegram Desktop did not answer within the capture deadline; restore the window or try method=screen"
            );
        }
    }
}
