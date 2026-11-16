use crate::backend::Telegram;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub fn required<'a>(args: &'a Value, key: &str) -> Result<&'a Value> {
    args.get(key)
        .filter(|value| !value.is_null())
        .with_context(|| format!("{key} is required"))
}
pub fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    required(args, key)?
        .as_str()
        .with_context(|| format!("{key} must be a string"))
}
pub fn integer(args: &Value, key: &str, default: i64) -> Result<i64> {
    match args.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(Value::Number(n)) => n
            .as_i64()
            .with_context(|| format!("{key} must be a signed integer")),
        Some(Value::String(n)) => n
            .parse()
            .with_context(|| format!("{key} must be an integer")),
        _ => bail!("{key} must be an integer"),
    }
}
pub fn boolean(args: &Value, key: &str, default: bool) -> Result<bool> {
    match args.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .with_context(|| format!("{key} must be a boolean")),
    }
}
pub fn limit(args: &Value, key: &str, default: i64, max: i64) -> Result<i64> {
    let n = integer(args, key, default)?;
    if n < 1 {
        bail!("{key} must be positive");
    }
    Ok(n.min(max))
}
pub async fn peer(api: &dyn Telegram, args: &Value, key: &str) -> Result<Value> {
    api.resolve_peer(required(args, key)?).await
}
pub fn random_id() -> Value {
    json!(rand::random::<i64>().to_string())
}
pub fn object(kind: &str, fields: Value) -> Value {
    let mut value = fields.as_object().cloned().unwrap_or_default();
    value.insert("_".into(), json!(kind));
    Value::Object(value)
}
