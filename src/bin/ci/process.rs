use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

fn read_bounded(mut pipe: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = pipe.read(&mut chunk)?;
        if count == 0 {
            return Ok(bytes);
        }
        if count > limit.saturating_sub(bytes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CI child output exceeded its byte budget",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn receive_output(
    receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    deadline: Instant,
) -> Result<Vec<u8>> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .context("CI output pipe did not close within its time budget")?
        .context("CI output could not be read within its byte budget")
}

pub fn capture(command: &mut Command, seconds: u64) -> Result<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .context("A required CI executable could not start")?;
    let stdout = child
        .stdout
        .take()
        .context("Child stdout pipe unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Child stderr pipe unavailable")?;
    let (out_sender, out_receiver) = mpsc::sync_channel(1);
    let (err_sender, err_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = out_sender.send(read_bounded(stdout, MAX_OUTPUT_BYTES));
    });
    thread::spawn(move || {
        let _ = err_sender.send(read_bounded(stderr, MAX_OUTPUT_BYTES));
    });
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().context("Cannot inspect CI child")? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(seconds) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("A CI child exceeded its time budget; no success was recorded");
        }
        thread::sleep(Duration::from_millis(25));
    };
    // A descendant can inherit a pipe after the immediate child exits. It must
    // not turn a bounded command into an unbounded reader join.
    let stdout = receive_output(out_receiver, deadline)?;
    let stderr = receive_output(err_receiver, deadline)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_process.rs"]
mod tests;

pub fn text(command: &mut Command, seconds: u64) -> Result<String> {
    let output = capture(command, seconds)?;
    if !output.status.success() {
        bail!("A required CI child failed; its private output was suppressed");
    }
    String::from_utf8(output.stdout).context("CI metadata is not UTF-8")
}

pub fn git(args: &[&str]) -> Result<String> {
    text(Command::new("git").args(args), 60)
}

pub fn digest(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(std::fs::read(path).context("Required digest input unavailable")?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Check the actual image format and machine before attaching target provenance.
/// File names and a successful cross-build command alone cannot prove its ABI.
pub fn verify_binary_target(path: &std::path::Path, target: &str) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    let (format, machine) = match target {
        "x86_64-unknown-linux-gnu" => ("elf", 62),
        "aarch64-unknown-linux-gnu" => ("elf", 183),
        "x86_64-apple-darwin" => ("macho", 0x01000007),
        "aarch64-apple-darwin" => ("macho", 0x0100000c),
        "x86_64-pc-windows-msvc" => ("pe", 0x8664),
        "aarch64-pc-windows-msvc" => ("pe", 0xaa64),
        _ => bail!("Unsupported native release target"),
    };
    let mut file = std::fs::File::open(path).context("Native release image is unavailable")?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)
        .context("Native release image header is truncated")?;
    let valid = match format {
        "elf" => {
            &header[..7] == b"\x7fELF\x02\x01\x01"
                && matches!(u16::from_le_bytes(header[16..18].try_into()?), 2 | 3)
                && u32::from(u16::from_le_bytes(header[18..20].try_into()?)) == machine
        }
        "macho" => {
            u32::from_le_bytes(header[..4].try_into()?) == 0xfeedfacf
                && u32::from_le_bytes(header[4..8].try_into()?) == machine
                && u32::from_le_bytes(header[12..16].try_into()?) == 2
        }
        "pe" if &header[..2] == b"MZ" => {
            let offset = u64::from(u32::from_le_bytes(header[60..64].try_into()?));
            let length = file.metadata()?.len();
            if offset < 64 || length.saturating_sub(offset) < 26 {
                false
            } else {
                file.seek(SeekFrom::Start(offset))?;
                let mut coff = [0u8; 26];
                file.read_exact(&mut coff)?;
                let optional_size = u64::from(u16::from_le_bytes(coff[20..22].try_into()?));
                let characteristics = u16::from_le_bytes(coff[22..24].try_into()?);
                &coff[..4] == b"PE\0\0"
                    && u32::from(u16::from_le_bytes(coff[4..6].try_into()?)) == machine
                    && characteristics & 2 != 0
                    && characteristics & 0x2000 == 0
                    && optional_size >= 112
                    && length.saturating_sub(offset + 24) >= optional_size
                    && u16::from_le_bytes(coff[24..26].try_into()?) == 0x20b
            }
        }
        _ => false,
    };
    anyhow::ensure!(valid, "Native release header does not match its target");
    Ok(())
}
