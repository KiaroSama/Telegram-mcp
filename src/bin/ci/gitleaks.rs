use super::process;
#[path = "gitleaks_scope.rs"]
mod scope;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const VERSION: &str = "8.30.1";
const CHECKSUM: &str = "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Result<Self> {
        let root = env::var_os("RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .canonicalize()?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = root.join(format!("telegram-gitleaks-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn executable(name: &str) -> Result<PathBuf> {
    for root in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = root.join(name);
        if path.is_file() {
            return path
                .canonicalize()
                .context("Required scanner executable cannot be resolved");
        }
    }
    anyhow::bail!("Required scanner executable is unavailable")
}

fn safe(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\n', '\r'], " ")
}

pub fn scan() -> Result<()> {
    scan_with(false)
}
pub fn scan_full_history() -> Result<()> {
    scan_with(true)
}

fn scan_with(full_history: bool) -> Result<()> {
    ensure!(
        env::consts::OS == "linux" && env::consts::ARCH == "x86_64",
        "The checksum-pinned scanner is for the Linux x64 CI runner"
    );
    ensure!(
        env::var("RUNNER_DEBUG").ok().as_deref() != Some("1"),
        "Disable Actions step debug logging before scanning; checkout diagnostics can disclose commit contents"
    );
    let temporary = Temporary::new()?;
    let archive = temporary.0.join("gitleaks.tar.gz");
    let url = format!(
        "https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/gitleaks_{VERSION}_linux_x64.tar.gz"
    );
    process::text(
        Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--tlsv1.2",
                "--max-time",
                "60",
                "--max-filesize",
                "100000000",
                "--output",
            ])
            .arg(&archive)
            .arg(url),
        70,
    )?;
    ensure!(
        process::digest(&archive)? == CHECKSUM,
        "The scanner download checksum does not match the pinned release"
    );
    let binary = unpack_verified_scanner(&archive, &temporary.0)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    }
    ensure!(
        process::text(Command::new(&binary).arg("version"), 15)?.trim() == VERSION,
        "Scanner executable version mismatch"
    );
    println!("Verified Gitleaks {VERSION} and its immutable download checksum.");
    let git_directory = process::git(&["rev-parse", "--absolute-git-dir"])?;
    let mut log_options =
        "--full-history --diff-filter=uxdb --no-ext-diff --no-textconv --ignore-submodules=all -m"
            .to_owned();
    let scan_scope = if full_history {
        log_options.push_str(" --all");
        "All fetched branches and tags, including full history and merge diffs"
    } else {
        let (range, description) = scope::introduced_range()?;
        log_options.push(' ');
        log_options.push_str(&range);
        description
    };
    let config = temporary.0.join("scanner.toml");
    fs::write(&config, "[extend]\nuseDefault = true\n")?;
    let report = temporary.0.join("findings.json");
    let status = temporary.0.join("git-status");
    let real_git = executable("git")?;
    let wrapper = temporary.0.join("git");
    fs::copy(env::current_exe()?, &wrapper)?;
    let path = env::join_paths(
        std::iter::once(temporary.0.clone())
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )?;
    if !full_history {
        let tree = temporary.0.join("tracked-tree");
        tracked_tree(&tree)?;
        let tree_report = temporary.0.join("tree-findings.json");
        let tree_output = process::capture(
            Command::new(&binary)
                .arg("dir")
                .arg(&tree)
                .arg("--config")
                .arg(&config)
                .arg("--gitleaks-ignore-path")
                .arg(&temporary.0)
                .args([
                    "--ignore-gitleaks-allow",
                    "--redact=100",
                    "--no-banner",
                    "--no-color",
                    "--log-level=error",
                    "--exit-code=2",
                    "--report-format=json",
                    "--report-path",
                ])
                .arg(&tree_report),
            300,
        )?;
        report_result(
            tree_output,
            &tree_report,
            "All regular files in the current tracked source tree",
        )?;
    }
    let output = process::capture(
        Command::new(&binary)
            .arg("git")
            .arg(git_directory.trim())
            .arg("--config")
            .arg(&config)
            .arg("--gitleaks-ignore-path")
            .arg(&temporary.0)
            .args([
                "--ignore-gitleaks-allow",
                "--redact=100",
                "--no-banner",
                "--no-color",
                "--log-level=error",
                "--exit-code=2",
                "--report-format=json",
                "--report-path",
            ])
            .arg(&report)
            .arg(format!("--log-opts={log_options}"))
            .env("PATH", path)
            .env("GITLEAKS_REAL_GIT", real_git)
            .env("GITLEAKS_GIT_STATUS", &status),
        930,
    )?;
    ensure!(
        fs::read_to_string(&status).ok().as_deref() == Some("0"),
        "The actual Git-history child did not confirm success"
    );
    report_result(output, &report, scan_scope)
}

fn unpack_verified_scanner(archive: &Path, destination: &Path) -> Result<PathBuf> {
    let listing = process::text(
        Command::new("tar")
            .arg("-tvzf")
            .arg(archive)
            .arg("gitleaks"),
        15,
    )?;
    ensure!(
        listing.starts_with('-') && listing.lines().count() == 1,
        "The verified archive member is not a regular executable file"
    );
    // The checksum-pinned executable is larger than the diagnostics budget.
    // Stream its archive member to a private file, never through captured stdout.
    process::text(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("--directory")
            .arg(destination)
            .args(["--no-same-owner", "--no-same-permissions", "--", "gitleaks"]),
        15,
    )?;
    let binary = destination.join("gitleaks");
    let metadata =
        fs::symlink_metadata(&binary).context("The verified scanner was not extracted")?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() > 0
            && metadata.len() <= 100_000_000,
        "The verified scanner executable is invalid"
    );
    Ok(binary)
}

fn tracked_tree(destination: &Path) -> Result<()> {
    let output = process::capture(Command::new("git").args(["ls-files", "-z"]), 60)?;
    ensure!(
        output.status.success() && !output.stdout.is_empty(),
        "The current tracked source cannot be enumerated"
    );
    fs::create_dir(destination)?;
    let mut total = 0u64;
    for bytes in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let name = std::str::from_utf8(bytes).context("A tracked source path is not UTF-8")?;
        let relative = Path::new(name);
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "A tracked path is not a contained relative source path"
        );
        let metadata =
            fs::symlink_metadata(relative).context("A tracked source file is missing")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "A tracked source is not a regular file"
        );
        total += metadata.len();
        ensure!(
            metadata.len() <= 32 * 1024 * 1024 && total <= 256 * 1024 * 1024,
            "The tracked source exceeds the bounded scan size"
        );
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().context("Invalid tracked source parent")?)?;
        fs::copy(relative, target)?;
    }
    Ok(())
}

fn report_result(output: std::process::Output, report: &Path, scan_scope: &str) -> Result<()> {
    ensure!(
        matches!(output.status.code(), Some(0 | 2)),
        "The scanner failed; no clean result was recorded"
    );
    ensure!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "The scanner reported errors; private output was withheld and no clean result was recorded"
    );
    let findings: Vec<Value> =
        serde_json::from_slice(&fs::read(report).context("The scanner did not produce a report")?)?;
    ensure!(
        findings.iter().all(Value::is_object),
        "The scanner report has an invalid shape"
    );
    ensure!(
        (output.status.code() == Some(2)) == !findings.is_empty(),
        "The scanner exit code and report disagree"
    );
    if let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") {
        let mut summary = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(
            summary,
            "## Gitleaks\n\nScanner: **{VERSION}**\n\nScope: **{scan_scope}**\n\nRules: pinned built-in Gitleaks rules with standard exclusions. Full history describes revision coverage, not universal byte or credential coverage.\n"
        )?;
        if findings.is_empty() {
            writeln!(
                summary,
                "No matching secrets were found within this scan scope."
            )?;
        } else {
            writeln!(
                summary,
                "**{} potential secrets detected.** Values, source snippets and commit messages are withheld.\n\n| Rule | File | Line | Commit |\n| --- | --- | --- | --- |",
                findings.len()
            )?;
            for finding in findings.iter().take(200) {
                writeln!(
                    summary,
                    "| <code>{}</code> | <code>{}</code> | <code>{}</code> | <code>{}</code> |",
                    safe(&finding["RuleID"]),
                    safe(&finding["File"]),
                    safe(&finding["StartLine"]),
                    safe(&finding["Commit"])
                )?;
            }
        }
    }
    ensure!(
        findings.is_empty(),
        "Gitleaks found {} potential secret(s); review redacted locations in the job summary",
        findings.len()
    );
    println!("The scan completed; no matching secrets were detected.");
    Ok(())
}

pub fn git_child() -> i32 {
    let result = (|| -> Result<i32> {
        let args: Vec<_> = env::args_os().skip(1).collect();
        let history = args.get(2).is_some_and(|arg| arg == "log");
        let mut child = Command::new(
            env::var_os("GITLEAKS_REAL_GIT").context("Actual Git executable missing")?,
        )
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
        let started = Instant::now();
        let code = loop {
            if let Some(status) = child.try_wait()? {
                break status.code().unwrap_or(1);
            }
            if started.elapsed() > Duration::from_secs(900) {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Git-history child timed out");
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        if history {
            fs::write(
                Path::new(
                    &env::var_os("GITLEAKS_GIT_STATUS").context("Git child status path missing")?,
                ),
                code.to_string(),
            )?;
        }
        Ok(code)
    })();
    match result {
        Ok(code) => code,
        Err(_) => {
            eprintln!("The verified Git-history child could not complete.");
            1
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_gitleaks.rs"]
mod tests;
