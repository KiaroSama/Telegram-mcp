//! Bounded reachable-blob credential scan, preserving the prior scanner's scope.
use super::process;
use anyhow::{Context, Result, bail, ensure};
use regex::bytes::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CHUNK: usize = 1 << 20;
const OVERLAP: usize = 1 << 16;
const MAX_OBJECT: u64 = 2 << 30;
const KNOWN_QT_ORACLE: &str = "23f836408ecffb26cbfe8e8a032102dd3dea0ee9";

fn rules() -> Result<Vec<(&'static str, Regex)>> {
    [
        ("Telegram bot token", r"\b\d{8,10}:[A-Za-z0-9_-]{30,45}\b"),
        (
            "Telegram authorization string",
            r"\b1[A-Za-z0-9+/_-]{200,}\b",
        ),
        ("private key block", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        ("AWS access key", r"\bAKIA[0-9A-Z]{16}\b"),
        ("GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
    ]
    .into_iter()
    .map(|(kind, pattern)| Ok((kind, Regex::new(pattern)?)))
    .collect()
}

fn body<R: Read>(
    input: &mut R,
    mut remaining: u64,
    scan: bool,
    rules: &[(&str, Regex)],
    deadline: Instant,
) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    let mut carry = Vec::new();
    let mut chunk = vec![0; CHUNK];
    while remaining > 0 {
        ensure!(
            Instant::now() < deadline,
            "The credential history scan exceeded its time budget"
        );
        let length = remaining.min(CHUNK as u64) as usize;
        input
            .read_exact(&mut chunk[..length])
            .context("A Git object body was truncated")?;
        remaining -= length as u64;
        if !scan {
            continue;
        }
        let mut window = carry;
        window.extend_from_slice(&chunk[..length]);
        let settled = if remaining == 0 {
            window.len()
        } else {
            window.len().saturating_sub(OVERLAP)
        };
        for (kind, rule) in rules {
            if rule.find_iter(&window).any(|hit| hit.end() <= settled) {
                found.insert((*kind).to_owned());
            }
        }
        let start = window.len().saturating_sub(OVERLAP * 2);
        carry = window[start..].to_vec();
    }
    Ok(found)
}

type Findings = Vec<(String, BTreeSet<String>)>;
fn stream<R: BufRead>(
    input: &mut R,
    expected: &BTreeSet<String>,
    deadline: Instant,
) -> Result<Findings> {
    let rules = rules()?;
    let mut seen = BTreeSet::new();
    let mut findings = Vec::new();
    loop {
        let mut header = Vec::new();
        let read = (&mut *input).take(4097).read_until(b'\n', &mut header)?;
        if read == 0 {
            break;
        }
        ensure!(
            header.len() <= 4096 && header.last() == Some(&b'\n'),
            "The Git-object header exceeds its bound or is incomplete"
        );
        let text = std::str::from_utf8(&header).context("A Git-object header is not UTF-8")?;
        let fields: Vec<_> = text.split_whitespace().collect();
        ensure!(
            fields.len() == 3,
            "Git returned a missing, ambiguous or malformed object"
        );
        let oid = fields[0];
        ensure!(
            expected.contains(oid) && seen.insert(oid.to_owned()),
            "Git returned an unexpected or duplicate object"
        );
        ensure!(
            matches!(fields[1], "blob" | "tree" | "commit" | "tag"),
            "Git returned an unknown object type"
        );
        let size: u64 = fields[2]
            .parse()
            .context("Git returned an invalid object size")?;
        ensure!(
            size <= MAX_OBJECT,
            "A Git object exceeds the scan size ceiling"
        );
        let mut kinds = body(input, size, fields[1] == "blob", &rules, deadline)?;
        let mut terminator = [0];
        input.read_exact(&mut terminator)?;
        ensure!(
            terminator == *b"\n",
            "A Git object has no record terminator"
        );
        // Exact historical fixture identity, verified from public Qt oracle pixels.
        // No path, rule family or other object receives this narrow exception.
        if oid == KNOWN_QT_ORACLE {
            kinds.remove("Telegram authorization string");
        }
        if !kinds.is_empty() {
            findings.push((oid.to_owned(), kinds));
        }
    }
    ensure!(
        seen == *expected,
        "Git stopped before every reachable object was scanned"
    );
    Ok(findings)
}

pub fn scan() -> Result<()> {
    let names = process::git(&["rev-list", "--objects", "--all"])?;
    let mut objects = BTreeMap::new();
    for row in names.lines() {
        let (oid, path) = row.split_once(' ').unwrap_or((row, ""));
        ensure!(
            matches!(oid.len(), 40 | 64) && oid.chars().all(|c| c.is_ascii_hexdigit()),
            "Git returned an invalid reachable object identity"
        );
        objects.insert(oid.to_owned(), path.to_owned());
    }
    ensure!(
        !objects.is_empty() && objects.len() <= 1_000_000,
        "The reachable object inventory is empty or exceeds its bound"
    );
    let expected: BTreeSet<_> = objects.keys().cloned().collect();
    let requests: String = expected.iter().map(|oid| format!("{oid}\n")).collect();
    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Git-object reader could not start")?;
    let mut stdin = child
        .stdin
        .take()
        .context("Git-object input pipe unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Git-object output pipe unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Git-object diagnostic pipe unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let writer = thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let reader = thread::spawn(move || stream(&mut BufReader::new(stdout), &expected, deadline));
    let diagnostics = thread::spawn(move || -> std::io::Result<bool> {
        let mut data = [0; 8192];
        let mut nonempty = false;
        loop {
            let length = stderr.read(&mut data)?;
            if length == 0 {
                break;
            }
            nonempty = true;
        }
        Ok(nonempty)
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Git-object reader exceeded the scan deadline; no clean result was recorded");
        }
        thread::sleep(Duration::from_millis(25));
    };
    ensure!(
        status.success(),
        "Git-object reader failed; private diagnostics were withheld"
    );
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Git-object input worker failed"))??;
    let findings = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Git-object reader worker failed"))??;
    ensure!(
        !diagnostics
            .join()
            .map_err(|_| anyhow::anyhow!("Git diagnostic worker failed"))??,
        "Git-object reader reported errors; no clean result was recorded"
    );
    for (oid, kinds) in &findings {
        let path = objects.get(oid).map(String::as_str).unwrap_or("");
        eprintln!(
            "History object {} at {}: potential {}; values withheld",
            &oid[..12],
            path,
            kinds.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    ensure!(
        findings.is_empty(),
        "Reachable blob history contains matching credential patterns"
    );
    println!(
        "Scanned all {} reachable Git objects with bounded buffers; no matching credential was found.",
        objects.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_history.rs"]
mod tests;
