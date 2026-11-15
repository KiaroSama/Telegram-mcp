use super::process;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    process::Command,
};

fn api(method: &str, path: &str, fields: Option<&Value>) -> Result<Value> {
    let mut command = Command::new("gh");
    command.args([
        "api",
        "--method",
        method,
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        path,
    ]);
    if let Some(fields) = fields {
        for (key, value) in fields.as_object().context("Invalid status payload")? {
            command.arg("-f").arg(format!(
                "{key}={}",
                value.as_str().context("Invalid status field")?
            ));
        }
    }
    let text = process::text(&mut command, 30)?;
    serde_json::from_str(&text).context("GitHub returned invalid status metadata")
}

fn variable(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("Required CI variable {name} is missing"))
}

pub fn publish(phase: &str, workflow: &str) -> Result<()> {
    ensure!(
        matches!(phase, "begin" | "finish"),
        "Invalid status reporting phase"
    );
    let repository = variable("GITHUB_REPOSITORY")?;
    let parts: Vec<_> = repository.split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|p| !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))),
        "Invalid status repository identity"
    );
    let sha = variable("VALIDATED_SHA")?;
    ensure!(
        sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
        "Invalid validated commit"
    );
    ensure!(
        process::git(&["rev-parse", "HEAD"])?.trim() == sha,
        "Status reporter checkout does not match validated commit"
    );
    let run_id: u64 = variable("GITHUB_RUN_ID")?.parse()?;
    let attempt: u64 = variable("GITHUB_RUN_ATTEMPT")?.parse()?;
    ensure!(run_id > 0 && attempt > 0, "Invalid CI run identity");
    let inventory: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&fs::read(".github/ci-status-jobs.json")?)?;
    let names = inventory
        .get(workflow)
        .context("This workflow has no expected CI job inventory")?;
    ensure!(
        !names.is_empty() && names.iter().collect::<BTreeSet<_>>().len() == names.len(),
        "Invalid expected CI job inventory"
    );
    let base = format!("repos/{repository}");
    let run_path = format!("{base}/actions/runs/{run_id}");
    let run = api("GET", &run_path, None)?;
    ensure!(
        run_matches(&run, &sha, attempt),
        "Workflow identity does not match this exact commit and attempt"
    );
    ensure!(
        run["path"].as_str() == Some(&format!(".github/workflows/{workflow}")),
        "Workflow path mismatch"
    );
    let run_url = format!("https://github.com/{repository}/actions/runs/{run_id}");
    let post = |name: &str, state: &str, outcome: &str, url: &str| -> Result<()> {
        let stem = workflow
            .strip_suffix(".yml")
            .context("Invalid workflow filename")?;
        let payload = json!({"context":format!("CI/{stem}/{name}"), "state":state, "description":format!("Run {run_id}, attempt {attempt}: {outcome}"), "target_url":url});
        api("POST", &format!("{base}/statuses/{sha}"), Some(&payload))?;
        println!("{name}: {state} ({outcome})");
        Ok(())
    };
    if phase == "begin" {
        for name in names {
            post(name, "pending", "awaiting observed job result", &run_url)?;
        }
        return Ok(());
    }
    let mut jobs = Vec::new();
    let mut complete = false;
    for page in 1..=10 {
        let response = api(
            "GET",
            &format!("{run_path}/attempts/{attempt}/jobs?per_page=100&page={page}"),
            None,
        )?;
        let batch = response["jobs"]
            .as_array()
            .context("GitHub jobs response is invalid")?;
        jobs.extend(batch.iter().cloned());
        if Some(jobs.len() as u64) == response["total_count"].as_u64() {
            complete = true;
            break;
        }
        ensure!(!batch.is_empty(), "GitHub job pagination is incomplete");
    }
    ensure!(complete, "GitHub jobs exceed bounded pagination");
    let mut observed = BTreeMap::new();
    let mut unexpected = false;
    for job in jobs {
        let name = job["name"]
            .as_str()
            .context("CI job has no name")?
            .to_owned();
        if matches!(
            name.as_str(),
            "CI status pending / Commit status publisher"
                | "CI status final / Commit status publisher"
        ) {
            continue;
        }
        if !names.contains(&name) || observed.insert(name, job).is_some() {
            unexpected = true;
        }
    }
    let mut failed = unexpected;
    for name in names {
        let mut state = "error";
        let mut outcome = "missing job";
        let mut url = run_url.as_str();
        if let Some(job) = observed.get(name) {
            let job_url = job["html_url"].as_str().unwrap_or("");
            let valid = job_matches(job, run_id, attempt, &sha, &run_url);
            if valid {
                url = job_url;
            }
            outcome = if !valid {
                "job identity mismatch"
            } else if unexpected {
                "unexpected or duplicate validation job"
            } else {
                let (observed_state, observed_outcome) = conclusion(job);
                state = observed_state;
                observed_outcome
            };
        }
        post(name, state, outcome, url)?;
        failed |= state != "success";
    }
    ensure!(
        !failed,
        "One or more validation jobs did not pass on this exact commit"
    );
    Ok(())
}

fn run_matches(run: &Value, sha: &str, attempt: u64) -> bool {
    run["head_sha"].as_str() == Some(sha) && run["run_attempt"].as_u64() == Some(attempt)
}
fn job_matches(job: &Value, run_id: u64, attempt: u64, sha: &str, run_url: &str) -> bool {
    let prefix = format!("{run_url}/job/");
    job["html_url"]
        .as_str()
        .and_then(|url| url.strip_prefix(&prefix))
        .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        && job["run_id"].as_u64() == Some(run_id)
        && job["head_sha"].as_str() == Some(sha)
        && job["run_attempt"].as_u64() == Some(attempt)
}
fn conclusion(job: &Value) -> (&'static str, &'static str) {
    match (job["status"].as_str(), job["conclusion"].as_str()) {
        (Some("completed"), Some("success")) => ("success", "success"),
        (Some("completed"), Some("failure")) => ("failure", "failure"),
        (Some("completed"), Some("timed_out")) => ("failure", "timed out"),
        (Some("completed"), Some("skipped")) => ("error", "skipped"),
        (Some("completed"), Some("cancelled")) => ("error", "cancelled"),
        _ => ("error", "not completed successfully"),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_status.rs"]
mod tests;
