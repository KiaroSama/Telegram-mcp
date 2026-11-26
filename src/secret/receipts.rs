use super::history::message_id;
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn eligible(row: &Value) -> bool {
    row.is_object()
        && row.get("can_be_saved").is_none_or(|v| v == true)
        && row
            .get("self_destructs_after_seconds")
            .is_none_or(|v| v.as_u64() == Some(0))
        && row.get("expired").is_none_or(|v| v == false)
        && row.get("expires_at").is_none_or(Value::is_null)
}

pub(super) fn retain_history(rows: &mut Vec<Value>) {
    rows.sort_by_cached_key(|row| {
        row["date"]
            .as_str()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
    });
    let mut seen = HashSet::new();
    rows.retain(|row| eligible(row) && message_id(row).is_none_or(|id| seen.insert(id)));
    rows.drain(..rows.len().saturating_sub(500));
}
