use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn request_fingerprint(tool: &str, args: &Value) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"telegram-mcp-approval-v1\0");
    hash.update(tool.as_bytes());
    hash.update(b"\0");
    hash.update(serde_json::to_vec(&canonical(args))?);
    Ok(crate::lowercase_hex(&hash.finalize()))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect();
            serde_json::to_value(sorted).expect("JSON object serializes")
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
