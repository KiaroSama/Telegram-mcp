use super::vision::{self, Fingerprint};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub fn known_pairs(path: &Path) -> Result<BTreeMap<String, String>> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("Cannot open the emoji catalog read-only")?;
    connection.pragma_update(None, "trusted_schema", false)?;
    let mut statement = connection
        .prepare("select custom_emoji_id, keywords from items where custom_emoji_id is not null")?;
    let mut rows = statement.query([])?;
    let mut pairs = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let own = match row.get_ref(0)? {
            ValueRef::Text(text) => std::str::from_utf8(text)?.to_owned(),
            ValueRef::Integer(number) => number.to_string(),
            _ => continue,
        };
        let keywords: String = row.get(1)?;
        let values: Vec<String> = serde_json::from_str(&keywords)
            .context("Catalog keywords must be a JSON string array")?;
        for keyword in values {
            if let Some(original) = keyword
                .strip_prefix("premium-id:")
                .filter(|id| *id != own && !id.is_empty())
            {
                pairs.insert(original.into(), own.clone());
            }
        }
    }
    Ok(pairs)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AccuracyReport {
    pub attempted: usize,
    pub resolved: usize,
    pub top_1: usize,
    pub top_3: usize,
    pub top_8: usize,
    pub top_20: usize,
    pub median_rank: Option<f64>,
    pub worst_rank: Option<usize>,
    pub unresolved: Vec<String>,
}

pub fn measure(
    pairs: &BTreeMap<String, String>,
    queries: &BTreeMap<String, Fingerprint>,
    index: &BTreeMap<String, Value>,
) -> AccuracyReport {
    let mut ranks = Vec::new();
    let mut unresolved = Vec::new();
    for (original, wanted) in pairs {
        let position = queries
            .get(original)
            .and_then(|query| {
                vision::rank(
                    query,
                    index,
                    index.len(),
                    &BTreeSet::from([original.clone()]),
                )
                .iter()
                .position(|(_, id)| id == wanted)
            })
            .map(|rank| rank + 1);
        match position {
            Some(rank) => ranks.push(rank),
            None => unresolved.push(original.clone()),
        }
    }
    ranks.sort_unstable();
    let len = ranks.len();
    let median_rank = if len == 0 {
        None
    } else if len % 2 == 1 {
        Some(ranks[len / 2] as f64)
    } else {
        Some((ranks[len / 2 - 1] + ranks[len / 2]) as f64 / 2.0)
    };
    AccuracyReport {
        attempted: pairs.len(),
        resolved: len,
        top_1: ranks.iter().filter(|r| **r <= 1).count(),
        top_3: ranks.iter().filter(|r| **r <= 3).count(),
        top_8: ranks.iter().filter(|r| **r <= 8).count(),
        top_20: ranks.iter().filter(|r| **r <= 20).count(),
        median_rank,
        worst_rank: ranks.last().copied(),
        unresolved,
    }
}
