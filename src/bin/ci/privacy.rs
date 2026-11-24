use super::process;
use anyhow::{Context, Result, ensure};
use regex::Regex;
use std::{fs, path::Path, process::Command};

const PLACEHOLDERS: &[&str] = &[
    "0123456789abcdef0123456789abcdef",
    "12345678901234567890123456789012",
    "abcdefabcdefabcdefabcdefabcdefab",
];
const GENERIC_NAMES: &[&str] = &[
    "user", "users", "you", "dev", "runner", "example", "username", "youruser", "john", "jane",
    "alice", "bob",
];

fn rules() -> Result<Vec<(&'static str, Regex)>> {
    [
        ("Telegram API hash", r"[0-9a-fA-F]{32}"),
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
    .map(|(name, pattern)| Ok((name, Regex::new(pattern)?)))
    .collect()
}

fn protected(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if normalized == ".agents/plugins/marketplace.json" {
        return false;
    }
    let parts: Vec<_> = normalized.split('/').collect();
    parts.iter().any(|p| {
        matches!(
            *p,
            ".ai"
                | ".claude"
                | ".kiro"
                | ".codex"
                | ".cursor"
                | ".cline"
                | ".agents"
                | "graphify-out"
                | "plans"
                | ".personal"
                | ".ignoreme"
                | "secrets.md"
                | "explain-AI.md"
        )
    }) || normalized.ends_with(".session")
        || normalized.ends_with(".session-journal")
        || parts.iter().any(|p| {
            *p == ".env"
                || (p.starts_with(".env.")
                    && !matches!(
                        *p,
                        ".env.example" | ".env.sample" | ".env.template" | ".env.dist"
                    ))
        })
}

fn problems(text: &str, rules: &[(&str, Regex)]) -> Result<Vec<(usize, String)>> {
    let mut findings = Vec::new();
    for (name, pattern) in rules {
        for capture in pattern.captures_iter(text) {
            let value = capture
                .get(1)
                .or_else(|| capture.get(0))
                .context("Credential matcher failed")?;
            if *name == "Telegram API hash"
                && (value
                    .start()
                    .checked_sub(1)
                    .and_then(|index| text.as_bytes().get(index))
                    .is_some_and(u8::is_ascii_hexdigit)
                    || text
                        .as_bytes()
                        .get(value.end())
                        .is_some_and(u8::is_ascii_hexdigit))
            {
                continue;
            }
            if PLACEHOLDERS.contains(&value.as_str()) {
                continue;
            }
            findings.push((
                text[..value.start()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1,
                (*name).to_owned(),
            ));
        }
    }
    for pattern in [
        r"[A-Za-z]:[\\/]Users[\\/]([A-Za-z0-9._-]+)",
        r"/home/([a-zA-Z0-9._-]+)",
    ] {
        for capture in Regex::new(pattern)?.captures_iter(text) {
            let value = capture.get(1).context("Private path matcher failed")?;
            if !GENERIC_NAMES.contains(&value.as_str().to_ascii_lowercase().as_str()) {
                findings.push((
                    text[..value.start()]
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count()
                        + 1,
                    "personal filesystem path".to_owned(),
                ));
            }
        }
    }
    Ok(findings)
}

pub fn scan() -> Result<()> {
    let output = process::capture(Command::new("git").args(["ls-files", "-z"]), 60)?;
    ensure!(
        output.status.success(),
        "The tracked source inventory is unavailable; no clean scan was recorded"
    );
    let rules = rules()?;
    let mut count = 0;
    let mut findings = 0;
    for raw_path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(raw_path).context("A tracked path is not UTF-8")?;
        count += 1;
        if protected(path) {
            eprintln!("Protected path is tracked: {path}");
            findings += 1;
        }
        let extension = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if matches!(extension, "py" | "pyc" | "js" | "ts" | "sh" | "bat" | "cmd") {
            eprintln!("Unmigrated executable source is tracked: {path}");
            findings += 1;
        }
        if extension == "ps1"
            && !(matches!(path, "start-mcp.ps1" | "Manage-Accounts.ps1")
                || path.starts_with("account-manager/")
                || path.starts_with("tests/"))
        {
            eprintln!("PowerShell source is outside the authorized launcher exception: {path}");
            findings += 1;
        }
        let bytes = fs::read(path).context("A tracked file is missing or unreadable")?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_)
                if matches!(
                    extension,
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "icc" | "ogg" | "tgs" | "zip"
                ) =>
            {
                continue;
            }
            Err(_) => anyhow::bail!("A tracked text or unknown-format file is not valid UTF-8"),
        };
        for (line, label) in problems(text, &rules)? {
            eprintln!("{path}:{line}: potential {label}; value withheld");
            findings += 1;
        }
    }
    ensure!(count > 0, "No tracked source was scanned");
    ensure!(
        findings == 0,
        "Publication privacy/native-source gate found {findings} issue(s)"
    );
    println!(
        "Scanned {count} tracked files; no matching credential/private path or unported executable source was found."
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_privacy.rs"]
mod tests;
