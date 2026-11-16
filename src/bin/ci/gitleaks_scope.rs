use super::process;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{env, fs};

pub(super) fn revision(value: Option<&str>) -> Result<&str> {
    let value = value.context("A Git revision is missing")?;
    ensure!(
        value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()),
        "Invalid Git revision"
    );
    Ok(value)
}

fn merge_base(head: &str) -> Result<String> {
    let base = process::git(&["merge-base", "origin/main", head])?
        .trim()
        .to_owned();
    revision(Some(&base))?;
    process::git(&["merge-base", "--is-ancestor", &base, head])?;
    Ok(base)
}

pub fn introduced_range() -> Result<(String, &'static str)> {
    let head = process::git(&["rev-parse", "HEAD"])?.trim().to_owned();
    revision(Some(&head))?;
    let event_name = env::var("GITHUB_EVENT_NAME").unwrap_or_default();
    let event = if let Some(path) = env::var_os("GITHUB_EVENT_PATH") {
        serde_json::from_slice::<Value>(&fs::read(path)?)?
    } else {
        Value::Null
    };
    let (base, scope) = if event_name == "pull_request" {
        let base = revision(event["pull_request"]["base"]["sha"].as_str())?;
        let requested_head = revision(event["pull_request"]["head"]["sha"].as_str())?;
        ensure!(
            requested_head == head,
            "PR introduction scan checkout differs from the requested head"
        );
        (
            base.to_owned(),
            "Commits introduced by this pull request, including merge diffs",
        )
    } else if event_name == "push" {
        let after = revision(event["after"].as_str())?;
        ensure!(
            after == head,
            "Push introduction scan checkout differs from the pushed head"
        );
        let before = revision(event["before"].as_str())?;
        if before.bytes().all(|byte| byte == b'0') {
            (
                merge_base(&head)?,
                "New branch commits since the verified merge-base with origin/main",
            )
        } else {
            (
                before.to_owned(),
                "Commits introduced by this push, including merge diffs",
            )
        }
    } else {
        (
            merge_base(&head)?,
            "Branch commits since the verified merge-base with origin/main",
        )
    };
    for sha in [&base, &head] {
        process::git(&["cat-file", "-e", &format!("{sha}^{{commit}}")])?;
    }
    Ok((format!("{base}..{head}"), scope))
}
