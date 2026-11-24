use crate::{args::text, backend::Telegram};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub const TOOL_NAMES: &[&str] = &[
    "configure_secret_backend",
    "get_secret_backend_status",
    "pause_secret_chat",
    "get_secret_pause_status",
    "check_secret_custody",
    "resume_secret_chat",
    "list_secret_deliveries",
    "acknowledge_secret_delivery",
    "receive_secret_media",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some(match payload(name, args) {
        Ok(payload) => api.secret_call(name, payload).await,
        Err(error) => Err(error),
    })
}

fn payload(name: &str, args: &Value) -> Result<Value> {
    let mut payload = args
        .as_object()
        .cloned()
        .context("Tool arguments must be an object")?;
    match name {
        "configure_secret_backend" => validate_config(args)?,
        "get_secret_backend_status" | "check_secret_custody" => {}
        _ => {
            let id = super::secret_id(args, "secret_chat_id")?;
            payload.insert("secret_chat_id".into(), json!(id));
            payload.insert("chat_id".into(), json!(id));
        }
    }
    match name {
        "list_secret_deliveries" => {
            payload.insert(
                "limit".into(),
                json!(bounded_int(args, "limit", 1, 100, Some(50))?),
            );
            payload.insert(
                "offset".into(),
                json!(bounded_int(args, "offset", 0, 4096, Some(0))?),
            );
        }
        "acknowledge_secret_delivery" => {
            bounded_int(args, "seq_no", 0, i64::from(i32::MAX), None)?;
        }
        "receive_secret_media" => {
            bounded_int(args, "message_id", i64::MIN, i64::MAX, None)?;
            bounded_int(args, "max_bytes", 1, 64 * 1024 * 1024, None)?;
        }
        _ => {}
    }
    Ok(Value::Object(payload))
}

fn bounded_int(args: &Value, key: &str, min: i64, max: i64, default: Option<i64>) -> Result<i64> {
    let value = match args.get(key) {
        None => default,
        Some(value) => value.as_i64(),
    };
    value
        .filter(|value| (min..=max).contains(value))
        .with_context(|| format!("{key} must be an integer from {min} to {max}"))
}

fn validate_config(args: &Value) -> Result<()> {
    let ints = [
        ("records", 4096, true),
        ("bytes", 64 * 1024 * 1024, true),
        ("chats", 64, false),
        ("frame_bytes", 1024 * 1024, false),
        ("tasks", 64, false),
    ];
    let numbers = [
        ("age_seconds", 2_592_000.0, true),
        ("operation_seconds", 60.0, false),
    ];
    match text(args, "mode")? {
        "normal" => ensure!(
            ints.iter().all(|(key, _, _)| args[*key].is_null())
                && numbers.iter().all(|(key, _, _)| args[*key].is_null()),
            "normal mode does not take custody budgets"
        ),
        "bounded" => {
            for (key, max, required) in ints {
                if required || !args[key].is_null() {
                    bounded_int(args, key, 1, max, None)?;
                }
            }
            for (key, max, required) in numbers {
                if required || !args[key].is_null() {
                    ensure!(
                        args[key]
                            .as_f64()
                            .is_some_and(|value| value > 0.0 && value <= max),
                        "{key} must be a positive number no greater than {max}"
                    );
                }
            }
        }
        _ => bail!("mode must be normal or bounded"),
    }
    Ok(())
}
