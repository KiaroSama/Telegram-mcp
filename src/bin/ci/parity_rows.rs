use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufWriter, Write},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RowKey {
    pub path: String,
    pub qualified_symbol: String,
    pub kind: String,
    pub line: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    Matched,
    VerifiedNativeEquivalent,
    ExplicitlyAuthorizedDifference,
    Unresolved,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InventoryFile {
    pub items: Vec<InventoryItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InventoryItem {
    pub file: String,
    pub qual: String,
    pub kind: String,
    pub line: u64,
    pub area: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParityRow {
    pub path: String,
    pub qualified_symbol: String,
    pub kind: String,
    pub line: u64,
    pub area: String,
    pub candidate_sha: String,
    pub owning_track: String,
    pub mapping: String,
    pub method: String,
    pub evidence: String,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerdictCounts {
    pub matched: usize,
    pub verified_native_equivalent: usize,
    pub explicitly_authorized_difference: usize,
    pub unresolved: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ParitySummary {
    pub candidate_sha: String,
    pub total: usize,
    pub verdicts: VerdictCounts,
    pub by_area: BTreeMap<String, VerdictCounts>,
}

impl InventoryItem {
    fn key(&self) -> RowKey {
        RowKey {
            path: self.file.clone(),
            qualified_symbol: self.qual.clone(),
            kind: self.kind.clone(),
            line: self.line,
        }
    }
}

impl ParityRow {
    pub(crate) fn key(&self) -> RowKey {
        RowKey {
            path: self.path.clone(),
            qualified_symbol: self.qualified_symbol.clone(),
            kind: self.kind.clone(),
            line: self.line,
        }
    }
}

fn valid_hex_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_baseline_status(status: &str) -> bool {
    matches!(
        status,
        "MATCHED" | "ARCHITECTURAL_EQUIV" | "PARTIAL" | "MISSING"
    )
}

fn is_unsupported_bridge(method: &str) -> bool {
    let lower = method.trim().to_ascii_lowercase();
    lower == "unsupported_bridge" || lower.starts_with("unsupported")
}

fn record_verdict(counts: &mut VerdictCounts, verdict: Verdict) {
    match verdict {
        Verdict::Matched => counts.matched += 1,
        Verdict::VerifiedNativeEquivalent => counts.verified_native_equivalent += 1,
        Verdict::ExplicitlyAuthorizedDifference => counts.explicitly_authorized_difference += 1,
        Verdict::Unresolved => counts.unresolved += 1,
    }
}

pub(crate) fn reconcile_rows(
    inventory: &InventoryFile,
    rows: &[ParityRow],
    expected_sha: &str,
    expected_areas: &BTreeSet<String>,
) -> Result<(Vec<ParityRow>, ParitySummary)> {
    ensure!(
        valid_hex_sha(expected_sha),
        "Expected candidate SHA must be 40 lowercase hex characters"
    );
    ensure!(
        !inventory.items.is_empty(),
        "Frozen parity inventory must not be empty"
    );

    let mut frozen: BTreeMap<RowKey, &InventoryItem> = BTreeMap::new();
    for item in &inventory.items {
        ensure!(
            valid_baseline_status(&item.status),
            "Unsupported baseline status `{}` for {}:{}",
            item.status,
            item.file,
            item.qual
        );
        ensure!(
            expected_areas.is_empty() || expected_areas.contains(&item.area),
            "Inventory area `{}` is outside the declared area set",
            item.area
        );
        let key = item.key();
        ensure!(
            frozen.insert(key.clone(), item).is_none(),
            "Duplicate frozen inventory key {}::{} ({}:{})",
            key.path,
            key.qualified_symbol,
            key.kind,
            key.line
        );
    }

    let mut seen: BTreeMap<RowKey, ParityRow> = BTreeMap::new();
    for row in rows {
        let key = row.key();
        let Some(inv) = frozen.get(&key) else {
            bail!(
                "Extra parity row not present in frozen inventory: {}::{} ({}:{})",
                key.path,
                key.qualified_symbol,
                key.kind,
                key.line
            );
        };
        ensure!(
            row.candidate_sha == expected_sha,
            "Candidate SHA mismatch on {}::{}: expected {}, got {}",
            key.path,
            key.qualified_symbol,
            expected_sha,
            row.candidate_sha
        );
        ensure!(
            row.area == inv.area,
            "Area mismatch on {}::{}: expected {}, got {}",
            key.path,
            key.qualified_symbol,
            inv.area,
            row.area
        );
        ensure!(
            !row.owning_track.trim().is_empty(),
            "Missing owning_track on {}::{}",
            key.path,
            key.qualified_symbol
        );
        ensure!(
            !row.method.trim().is_empty(),
            "Missing verification method on {}::{}",
            key.path,
            key.qualified_symbol
        );

        let is_match = matches!(
            row.verdict,
            Verdict::Matched | Verdict::VerifiedNativeEquivalent
        );
        if is_match {
            ensure!(
                !is_unsupported_bridge(&row.method),
                "Unsupported bridge cannot pass as a match on {}::{}",
                key.path,
                key.qualified_symbol
            );
            ensure!(
                !row.evidence.trim().is_empty() && !row.mapping.trim().is_empty(),
                "Behavioral evidence and mapping are required to mark {}::{} as matched",
                key.path,
                key.qualified_symbol
            );
            ensure!(
                row.authorization_ref.is_none(),
                "Authorized difference must not be counted as a match on {}::{}",
                key.path,
                key.qualified_symbol
            );
        } else if row.verdict == Verdict::ExplicitlyAuthorizedDifference {
            ensure!(
                row.authorization_ref
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty()),
                "Explicitly authorized difference requires authorization_ref on {}::{}",
                key.path,
                key.qualified_symbol
            );
            ensure!(
                !row.evidence.trim().is_empty(),
                "Explicitly authorized difference requires evidence on {}::{}",
                key.path,
                key.qualified_symbol
            );
        }

        ensure!(
            seen.insert(key.clone(), row.clone()).is_none(),
            "Duplicate parity row for {}::{} ({}:{})",
            key.path,
            key.qualified_symbol,
            key.kind,
            key.line
        );
    }

    let missing: Vec<_> = frozen.keys().filter(|k| !seen.contains_key(*k)).collect();
    ensure!(
        missing.is_empty(),
        "Missing {} frozen parity keys (first: {}::{})",
        missing.len(),
        missing[0].path,
        missing[0].qualified_symbol
    );

    let mut verdicts = VerdictCounts::default();
    let mut by_area: BTreeMap<String, VerdictCounts> = BTreeMap::new();
    let mut sorted = Vec::with_capacity(seen.len());
    for (_, row) in seen {
        record_verdict(&mut verdicts, row.verdict);
        record_verdict(by_area.entry(row.area.clone()).or_default(), row.verdict);
        sorted.push(row);
    }

    if !expected_areas.is_empty() {
        for area in expected_areas {
            ensure!(
                by_area.contains_key(area),
                "Declared parity area `{area}` has zero reconciled rows"
            );
        }
    }

    let summary = ParitySummary {
        candidate_sha: expected_sha.to_owned(),
        total: sorted.len(),
        verdicts,
        by_area,
    };
    Ok((sorted, summary))
}

pub(crate) fn compare_replays(first: &[ParityRow], second: &[ParityRow]) -> Result<()> {
    ensure!(
        first.len() == second.len(),
        "Replay row count diverged: {} vs {}",
        first.len(),
        second.len()
    );
    for (idx, (left, right)) in first.iter().zip(second.iter()).enumerate() {
        ensure!(
            left.key() == right.key(),
            "Replay key order diverged at row {idx}: {}::{} vs {}::{}",
            left.path,
            left.qualified_symbol,
            right.path,
            right.qualified_symbol
        );
        ensure!(
            left.candidate_sha == right.candidate_sha && left.area == right.area,
            "Replay metadata diverged on {}::{}",
            left.path,
            left.qualified_symbol
        );
        ensure!(
            left.verdict == right.verdict,
            "Nondeterministic verdict on {}::{}: {:?} vs {:?}",
            left.path,
            left.qualified_symbol,
            left.verdict,
            right.verdict
        );
    }
    Ok(())
}

pub(crate) fn read_jsonl(path: &Path) -> Result<Vec<ParityRow>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Unable to read parity JSONL {}", path.display()))?;
    let mut rows = Vec::new();
    for (index, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row: ParityRow = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "Invalid parity row JSON at line {} in {}",
                index + 1,
                path.display()
            )
        })?;
        rows.push(row);
    }
    Ok(rows)
}

pub(crate) fn reconcile_files(
    inventory_path: &Path,
    evidence_path: &Path,
    output_path: &Path,
    expected_sha: &str,
) -> Result<ParitySummary> {
    let inventory: InventoryFile = serde_json::from_str(
        &fs::read_to_string(inventory_path)
            .with_context(|| format!("Unable to read inventory {}", inventory_path.display()))?,
    )
    .context("Invalid parity inventory JSON")?;
    let rows = read_jsonl(evidence_path)?;
    let areas: BTreeSet<String> = inventory.items.iter().map(|i| i.area.clone()).collect();
    let (sorted, summary) = reconcile_rows(&inventory, &rows, expected_sha, &areas)?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(output_path)
        .with_context(|| format!("Unable to create {}", output_path.display()))?;
    let mut writer = BufWriter::new(file);
    for row in &sorted {
        serde_json::to_writer(&mut writer, row)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(summary)
}

pub(crate) fn verify_replay_files(first_path: &Path, second_path: &Path) -> Result<usize> {
    let first = read_jsonl(first_path)?;
    let second = read_jsonl(second_path)?;
    compare_replays(&first, &second)?;
    Ok(first.len())
}
