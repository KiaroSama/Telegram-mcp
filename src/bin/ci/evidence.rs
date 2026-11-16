use super::process;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeLeg {
    pub platform: String,
    pub architecture: String,
    pub target: String,
}

const NATIVE_TARGETS: [(&str, &str, &str); 6] = [
    ("linux", "x86_64", "x86_64-unknown-linux-gnu"),
    ("linux", "aarch64", "aarch64-unknown-linux-gnu"),
    ("macos", "x86_64", "x86_64-apple-darwin"),
    ("macos", "aarch64", "aarch64-apple-darwin"),
    ("windows", "x86_64", "x86_64-pc-windows-msvc"),
    ("windows", "aarch64", "aarch64-pc-windows-msvc"),
];

pub(super) fn native_legs() -> Result<BTreeMap<String, NativeLeg>> {
    let legs = serde_json::from_slice(&fs::read(".github/expected-native-legs.json")?)?;
    validate_leg_manifest(&legs)?;
    Ok(legs)
}

fn validate_leg_manifest(legs: &BTreeMap<String, NativeLeg>) -> Result<()> {
    let mut required = BTreeMap::new();
    for (platform, architecture, target) in NATIVE_TARGETS {
        let leg = NativeLeg {
            platform: platform.into(),
            architecture: architecture.into(),
            target: target.into(),
        };
        for prefix in ["", "renderer-"] {
            required.insert(format!("{prefix}{platform}-{architecture}"), leg.clone());
        }
    }
    ensure!(
        legs == &required,
        "Native evidence must include both architectures and renderers for all three operating systems"
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    schema_version: u32,
    label: String,
    platform: String,
    architecture: String,
    target: String,
    rustc: String,
    commit: String,
    lock_sha256: String,
    total: usize,
    failed: usize,
    skipped: usize,
    cases: Vec<String>,
    executed: Vec<String>,
}

pub(super) fn summary(message: &str) -> Result<()> {
    use std::io::Write;
    println!("{message}");
    if let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") {
        writeln!(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?,
            "{message}"
        )?;
    }
    Ok(())
}

pub fn record(label: &str, junit: &Path, destination: &Path) -> Result<()> {
    ensure!(
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
        "Invalid CI leg label"
    );
    let document =
        fs::read_to_string(junit).context("The test report is missing or is not UTF-8")?;
    let (cases, executed, failed, skipped) = parse_report(&document)?;
    let commit = process::git(&["rev-parse", "HEAD"])?.trim().to_owned();
    if let Ok(expected) = env::var("VALIDATED_SHA") {
        ensure!(
            expected == commit,
            "The tested checkout is not the requested commit"
        );
    }
    let compiler = process::text(Command::new("rustc").arg("-vV"), 15)?;
    let target = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("The actual native compiler host is missing")?;
    ensure!(
        NATIVE_TARGETS.contains(&(env::consts::OS, env::consts::ARCH, target)),
        "The case recorder is not using a supported native compiler host"
    );
    let record = Record {
        schema_version: 2,
        label: label.to_owned(),
        platform: env::consts::OS.to_owned(),
        architecture: env::consts::ARCH.to_owned(),
        target: target.to_owned(),
        rustc: compiler
            .lines()
            .next()
            .context("Native compiler version is missing")?
            .to_owned(),
        commit,
        lock_sha256: process::digest(Path::new("Cargo.lock"))?,
        total: cases.len(),
        failed,
        skipped,
        cases,
        executed,
    };
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(destination, serde_json::to_vec_pretty(&record)?)?;
    summary(&format!(
        "### {}\n\nCommit: `{}`\n\n{} native tests, {} failed, {} skipped; {} / {} / `{}`.\n",
        label,
        record.commit,
        record.total,
        failed,
        skipped,
        record.platform,
        record.architecture,
        record.target
    ))?;
    require_complete_execution(&record)?;
    ensure!(
        native_legs()?.get(label)
            == Some(&NativeLeg {
                platform: record.platform,
                architecture: record.architecture,
                target: record.target,
            }),
        "The actual native compiler host does not match this reviewed CI architecture label"
    );
    Ok(())
}

fn require_complete_execution(record: &Record) -> Result<()> {
    ensure!(
        record.failed == 0,
        "Native tests failed; their complete case record was retained"
    );
    ensure!(
        record.skipped == 0 && record.executed == record.cases,
        "Native tests were skipped or not executed; their complete case record was retained"
    );
    Ok(())
}

fn parse_report(document: &str) -> Result<(Vec<String>, Vec<String>, usize, usize)> {
    let xml = roxmltree::Document::parse(document).context("The test report is not valid XML")?;
    let mut cases = BTreeSet::new();
    let mut executed = BTreeSet::new();
    let mut failed = 0;
    let mut skipped = 0;
    for suite in xml.descendants().filter(|n| n.has_tag_name("testsuite")) {
        let suite_name = suite
            .attribute("name")
            .context("JUnit suite has no identity")?;
        let members: Vec<_> = suite
            .children()
            .filter(|n| n.has_tag_name("testcase"))
            .collect();
        if members.is_empty() {
            continue;
        }
        let declared: usize = suite
            .attribute("tests")
            .context("JUnit suite has no declared count")?
            .parse()?;
        ensure!(
            declared == members.len(),
            "JUnit count disagrees with the actual test cases"
        );
        for case in members {
            let name = case.attribute("name").context("JUnit test has no name")?;
            let identity = format!("{suite_name}::{name}");
            ensure!(
                cases.insert(identity.clone()),
                "Duplicate JUnit test identity"
            );
            if case.children().any(|n| n.has_tag_name("skipped")) {
                skipped += 1;
            } else {
                executed.insert(identity);
            }
            if case
                .children()
                .any(|n| n.has_tag_name("failure") || n.has_tag_name("error"))
            {
                failed += 1;
            }
        }
    }
    ensure!(
        !cases.is_empty(),
        "An empty test report is not a passing test run"
    );
    Ok((
        cases.into_iter().collect(),
        executed.into_iter().collect(),
        failed,
        skipped,
    ))
}

fn records(directory: &Path) -> Result<Vec<Record>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(directory).context("Native test case records are unavailable")? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let record: Record = serde_json::from_slice(&fs::read(path)?)?;
            ensure!(
                record.schema_version == 2
                    && record.total > 0
                    && record.cases.len() == record.total,
                "Invalid native test record"
            );
            result.push(record);
        }
    }
    ensure!(
        !result.is_empty(),
        "No native test case record was produced"
    );
    Ok(result)
}

pub fn compare(directory: &Path) -> Result<()> {
    let records = validated_records(directory)?;
    let checkout = process::git(&["rev-parse", "HEAD"])?.trim().to_owned();
    let mut counts = BTreeMap::new();
    for record in &records {
        counts.insert(record.platform.as_str(), record.total);
    }
    let counts = counts
        .iter()
        .map(|(platform, count)| format!("{platform}: {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    summary(&format!(
        "### Native case agreement\n\nAll {} base/renderer architecture legs match their reviewed operating-system inventories at `{checkout}`. Counts: {counts}. Both ARM64 and x86_64 compiler hosts and target identities were verified; platform-specific Rust cfg cases remain explicit.\n",
        records.len()
    ))
}

fn validated_records(directory: &Path) -> Result<Vec<Record>> {
    let expected = native_legs()?;
    let records = records(directory)?;
    let checkout = process::git(&["rev-parse", "HEAD"])?.trim().to_owned();
    let lock = process::digest(Path::new("Cargo.lock"))?;
    let expected_cases: BTreeMap<String, Vec<String>> = serde_json::from_slice(
        &fs::read(".github/expected-native-tests.json")
            .context("Expected native case inventory is missing")?,
    )?;
    validate_record_set(&records, &expected, &expected_cases, &checkout, &lock)?;
    Ok(records)
}

fn validate_record_set(
    records: &[Record],
    expected: &BTreeMap<String, NativeLeg>,
    expected_cases: &BTreeMap<String, Vec<String>>,
    checkout: &str,
    lock: &str,
) -> Result<()> {
    validate_leg_manifest(expected)?;
    ensure!(
        !expected_cases.is_empty(),
        "Expected native case inventory is empty"
    );
    let mut actual = BTreeMap::new();
    for record in records {
        ensure!(
            record.schema_version == 2 && record.total > 0 && record.total == record.cases.len(),
            "Invalid native case record version or count"
        );
        ensure!(
            actual
                .insert(
                    record.label.clone(),
                    NativeLeg {
                        platform: record.platform.clone(),
                        architecture: record.architecture.clone(),
                        target: record.target.clone()
                    }
                )
                .is_none(),
            "Duplicate CI leg record"
        );
        ensure!(
            record.commit == checkout && record.lock_sha256 == lock,
            "A test record belongs to a different commit or lockfile"
        );
        ensure!(
            expected_cases.get(&record.platform) == Some(&record.cases),
            "A native test case was added, lost or renamed without updating its reviewed operating-system inventory"
        );
        require_complete_execution(record)?;
        ensure!(
            record.rustc.split_whitespace().nth(1) == Some("1.100.0"),
            "A CI leg used a different Rust compiler"
        );
    }
    ensure!(
        &actual == expected,
        "Native records are missing a required architecture, contain a mislabeled compiler target, or belong to the wrong native runner"
    );
    Ok(())
}

pub fn parity(directory: &Path) -> Result<()> {
    let ledger: serde_json::Value = serde_json::from_slice(
        &fs::read(".github/native-verification.json")
            .context("Native verification category ledger is unavailable")?,
    )?;
    ensure!(
        ledger["schema_version"].as_u64() == Some(1),
        "Unsupported native verification ledger"
    );
    let categories = ledger["categories"]
        .as_object()
        .context("Native verification categories are missing")?;
    let native: BTreeSet<String> = validated_records(directory)?
        .into_iter()
        .flat_map(|r| r.executed)
        .collect();
    let required = [
        "tool_surface",
        "protocol_codec",
        "message_behavior",
        "media_transfer",
        "identity_permissions",
        "account_state",
        "filesystem_boundaries",
        "human_approval",
        "event_lifecycle",
        "visual_rendering",
        "chat_export",
        "publication_privacy",
        "ci_evidence",
        "advanced_api",
        "plugin_distribution",
    ];
    for category in required {
        let checks = categories
            .get(category)
            .and_then(serde_json::Value::as_array)
            .with_context(|| {
                format!("Required native verification category is missing: {category}")
            })?;
        ensure!(
            !checks.is_empty(),
            "A native verification category declares no behavioral evidence"
        );
        for check in checks {
            let name = check
                .as_str()
                .context("A native verification selector is not a test name")?;
            let suffix = format!("::{name}");
            ensure!(
                native.iter().filter(|id| id.ends_with(&suffix)).count() == 1,
                "Required native behavioral test did not execute exactly once: {name}"
            );
        }
    }
    summary(&format!(
        "### Native behavioral verification\n\nAll {} required behavior categories have actual executed JUnit evidence. The historical 6,117 Python cases remain an audit baseline; exact count parity was explicitly waived. Category membership is traceability, not a substitute for reviewing assertions.\n",
        required.len()
    ))
}

pub fn package(target: &str) -> Result<()> {
    ensure!(
        !target.is_empty()
            && target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "Invalid package target"
    );
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let destination = Path::new("dist").join(target);
    let source = Path::new("target").join(target).join("release");
    let metadata = serde_json::json!({"commit":process::git(&["rev-parse","HEAD"])?.trim(),"target":target,"lock_sha256":process::digest(Path::new("Cargo.lock"))?,"rustc":process::text(Command::new("rustc").arg("--version"),15)?.trim()});
    package_files(&source, &destination, Path::new("."), suffix, metadata)?;
    summary(&format!(
        "Packaged all three actual `{target}` native CLIs with binary hashes and commit/lock/compiler provenance."
    ))
}

fn package_files(
    source: &Path,
    destination: &Path,
    repository: &Path,
    suffix: &str,
    mut metadata: serde_json::Value,
) -> Result<()> {
    ensure!(
        !destination.exists(),
        "Release package destination already exists; stale or owner files will not be overwritten"
    );
    let target = metadata["target"]
        .as_str()
        .context("Release package provenance has no target triple")?;
    let mut binaries = serde_json::Map::new();
    for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
        let filename = format!("{name}{suffix}");
        let path = source.join(&filename);
        let file = fs::symlink_metadata(&path)
            .context("A required native release executable is missing")?;
        ensure!(
            file.is_file() && !file.file_type().is_symlink() && file.len() > 0,
            "A native release executable is invalid"
        );
        process::verify_binary_target(&path, target)?;
        binaries.insert(name.into(), serde_json::json!({"filename":filename,"sha256":process::digest(&path)?,"bytes":file.len()}));
    }
    fs::create_dir_all(destination)?;
    for entry in binaries.values() {
        let filename = entry["filename"]
            .as_str()
            .expect("Validated binary filename");
        fs::copy(source.join(filename), destination.join(filename))?;
        ensure!(
            Some(process::digest(&destination.join(filename))?.as_str())
                == entry["sha256"].as_str(),
            "Copied native executable digest mismatch"
        );
    }
    for file in ["LICENSE", "NOTICE", "README.md"] {
        fs::copy(repository.join(file), destination.join(file))?;
    }
    metadata["binaries"] = serde_json::Value::Object(binaries);
    fs::write(
        destination.join("build.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_evidence.rs"]
mod tests;
