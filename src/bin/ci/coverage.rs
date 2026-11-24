use super::evidence::{native_legs, summary};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

pub fn run(directory: &Path) -> Result<()> {
    let mut lines: BTreeMap<(String, u64), bool> = BTreeMap::new();
    let scope: CoverageScope = serde_json::from_slice(&fs::read(".github/coverage-scope.json")?)?;
    scope.validate()?;
    scope.validate_source_tree(Path::new("."))?;
    let expected = native_legs()?;
    let mut report_labels = BTreeSet::new();
    let mut source_lines = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.extension().is_some_and(|x| x == "info") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|v| v.to_str())
            .context("Invalid coverage report name")?;
        let label = name
            .strip_prefix("coverage-")
            .context("Invalid coverage report label")?;
        ensure!(
            expected.contains_key(label) && report_labels.insert(label.to_owned()),
            "Unexpected or duplicate native coverage report"
        );
        let text = fs::read_to_string(path).context("Coverage report is not UTF-8")?;
        let project = env::var("GITHUB_REPOSITORY")
            .ok()
            .and_then(|r| r.split('/').next_back().map(str::to_owned))
            .unwrap_or_else(|| "telegram-mcp".into());
        let report = parse_lcov_report(&text, &project)?;
        ensure!(
            !report.is_empty(),
            "A coverage leg contains no executable project source lines"
        );
        for (key, hit) in report {
            scope.validate_source_line(Path::new("."), &key.0, key.1, &mut source_lines)?;
            *lines.entry(key).or_default() |= hit;
        }
    }
    ensure!(
        report_labels == expected.keys().cloned().collect() && !lines.is_empty(),
        "Coverage requires a nonempty report from every base and renderer architecture leg"
    );
    let all_hits = lines.values().filter(|hit| **hit).count();
    let all_percent = all_hits as f64 * 100.0 / lines.len() as f64;
    let measured: Vec<_> = lines
        .iter()
        .filter(|((path, _), _)| scope.include.contains_key(path))
        .collect();
    ensure!(
        !measured.is_empty(),
        "Reviewed core coverage contains no executable source lines"
    );
    let hits = measured.iter().filter(|(_, hit)| **hit).count();
    let percent = hits as f64 * 100.0 / measured.len() as f64;
    summary(&format!(
        "### Combined native line coverage\n\nAll production source: {all_hits}/{} executable lines, **{all_percent:.2}%**.\n\nReviewed application core: {hits}/{} executable lines, **{percent:.2}%** (required: 85%). Both metrics combine the eight base and renderer reports from ARM64 and x86_64 on Linux and Windows. `.github/coverage-scope.json` records each included/excluded path and its legacy measurement basis; full-source evidence remains available.\n",
        lines.len(),
        measured.len()
    ))?;
    ensure!(
        hits * 100 >= measured.len() * 85,
        "Combined reviewed application-core source-line coverage is below 85%"
    );
    Ok(())
}

#[derive(Deserialize)]
struct CoverageScope {
    schema_version: u32,
    legacy_commit: String,
    legacy_pyproject_sha256: String,
    floor_percent: u32,
    include: BTreeMap<String, String>,
    exclude: BTreeMap<String, String>,
}

impl CoverageScope {
    fn validate_source_line(
        &self,
        root: &Path,
        path: &str,
        line: u64,
        source_lines: &mut BTreeMap<String, u64>,
    ) -> Result<()> {
        ensure!(
            self.include.contains_key(path) || self.exclude.contains_key(path),
            "Coverage contains an unreviewed source path: {path}"
        );
        ensure!(line > 0, "Coverage source line cannot be zero");
        let length = match source_lines.entry(path.to_owned()) {
            std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let source = fs::read_to_string(root.join(path))
                    .context("Reviewed coverage source is missing or is not UTF-8")?;
                *entry.insert(u64::try_from(source.lines().count())?)
            }
        };
        ensure!(
            line <= length,
            "Coverage refers to a source line outside the current file: {path}:{line}; remove stale workspace objects before measuring coverage"
        );
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.floor_percent == 85,
            "Invalid coverage policy version or changed quality floor"
        );
        ensure!(
            self.legacy_commit.len() == 40
                && self.legacy_commit.bytes().all(|x| x.is_ascii_hexdigit())
                && self.legacy_pyproject_sha256.len() == 64
                && self
                    .legacy_pyproject_sha256
                    .bytes()
                    .all(|x| x.is_ascii_hexdigit()),
            "Coverage policy lacks its exact legacy basis"
        );
        ensure!(
            !self.include.is_empty(),
            "Coverage policy has no measured source paths"
        );
        for (path, reason) in self.include.iter().chain(&self.exclude) {
            ensure!(
                owned_source(path, "telegram-mcp").as_deref() == Some(path.as_str())
                    && !reason.trim().is_empty(),
                "Coverage policy has an invalid source path or unexplained scope decision"
            );
            ensure!(
                !(self.include.contains_key(path) && self.exclude.contains_key(path)),
                "Coverage policy both includes and excludes a source path"
            );
        }
        Ok(())
    }

    fn validate_source_tree(&self, root: &Path) -> Result<()> {
        let mut pending = vec![root.join("src")];
        let mut actual = BTreeSet::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                ensure!(
                    !kind.is_symlink(),
                    "Source tree contains an unreviewable symbolic link"
                );
                let path = entry.path();
                if kind.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    actual.insert(
                        path.strip_prefix(root)?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        if root.join("build.rs").is_file() {
            actual.insert("build.rs".into());
        }
        let reviewed: BTreeSet<_> = self
            .include
            .keys()
            .chain(self.exclude.keys())
            .cloned()
            .collect();
        ensure!(
            actual == reviewed,
            "Native source inventory changed without an explicit coverage-scope review"
        );
        Ok(())
    }
}

fn parse_lcov_report(text: &str, project: &str) -> Result<BTreeMap<(String, u64), bool>> {
    let mut result = BTreeMap::new();
    let mut file = None;
    for row in text.lines() {
        if let Some(name) = row.strip_prefix("SF:") {
            file = owned_source(name, project);
        } else if row == "end_of_record" {
            file = None;
        } else if let Some(data) = row.strip_prefix("DA:")
            && let Some(file) = &file
        {
            let mut parts = data.split(',');
            let line: u64 = parts.next().context("Coverage line missing")?.parse()?;
            let count: u64 = parts
                .next()
                .context("Coverage hit count missing")?
                .parse()?;
            ensure!(line > 0, "Coverage source line cannot be zero");
            *result.entry((file.clone(), line)).or_default() |= count > 0;
        }
    }
    Ok(result)
}

fn owned_source(name: &str, project: &str) -> Option<String> {
    let normalized = name.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let marker = format!("/{}/", project.to_ascii_lowercase());
    let relative = if normalized.starts_with("src/") || normalized == "build.rs" {
        normalized.as_str()
    } else {
        let index = lower.rfind(&marker)?;
        &normalized[index + marker.len()..]
    };
    ((relative.starts_with("src/") || relative == "build.rs")
        && relative.ends_with(".rs")
        && !relative.split('/').any(|part| part == ".."))
    .then(|| relative.to_owned())
}

#[cfg(test)]
#[path = "../../../tests/unit/ci_coverage.rs"]
mod tests;
