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
    Ok(format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).context("Required digest input unavailable")?)
    ))
}
