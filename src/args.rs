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

pub const MAX_PAGE_OFFSET: usize = 5_000;

pub const TIMING_BOUNDS: &[(&str, f64, f64)] = &[
    ("timeout", 0.1, 300.0),
    ("settle_ms", 50.0, 300_000.0),
    ("max_wait_ms", 100.0, 300_000.0),
    ("lock_grace_seconds", 0.0, 300.0),
    ("lock_poll_interval", 0.01, 60.0),
    ("wait_seconds", 0.0, 30.0),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub value: usize,
    pub requested: i64,
    pub ceiling: usize,
    pub error: Option<String>,
}

impl Bound {
    pub fn clamped(&self) -> bool {
        self.error.is_none() && (self.value as i64) != self.requested
    }

    pub fn metadata(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("requested_limit".into(), json!(self.requested));
        map.insert("effective_limit".into(), json!(self.value));
        if self.clamped() {
            map.insert(
                "limit_note".into(),
                json!(format!(
                    "Asked for {}; this tool serves at most {}, so {} were requested from Telegram. There may be more beyond them.",
                    self.requested, self.ceiling, self.value
                )),
            );
        }
        Value::Object(map)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub value: f64,
    pub error: Option<String>,
}

fn as_whole_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(_) | Value::Null | Value::Array(_) | Value::Object(_) => None,
        Value::Number(n) => n.as_i64().or_else(|| {
            let f = n.as_f64()?;
            (f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64)
                .then_some(f as i64)
        }),
        Value::String(s) => s.trim().parse::<i64>().ok(),
    }
}

fn as_finite_number(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Bool(_) | Value::Null | Value::Array(_) | Value::Object(_) => return None,
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
    };
    n.is_finite().then_some(n)
}

pub fn bounded(value: &Value, ceiling: usize, name: &str) -> Result<Bound> {
    if ceiling == 0 {
        bail!("ceiling for {name} must be a positive integer, got {ceiling}");
    }
    let Some(requested) = as_whole_int(value) else {
        return Ok(Bound {
            value: 0,
            requested: 0,
            ceiling,
            error: Some(format!(
                "Error: {name} must be a whole number, not {value}. Nothing was fetched."
            )),
        });
    };
    if requested < 1 {
        return Ok(Bound {
            value: 0,
            requested,
            ceiling,
            error: Some(format!(
                "Error: {name} must be at least 1; {requested} was given. Nothing was fetched."
            )),
        });
    }
    let effective = (requested as usize).min(ceiling);
    Ok(Bound {
        value: effective,
        requested,
        ceiling,
        error: None,
    })
}

pub fn bounded_page(
    page: &Value,
    page_size: &Value,
    ceiling: usize,
) -> Result<(Bound, Option<usize>)> {
    let size = bounded(page_size, ceiling, "page_size")?;
    if size.error.is_some() {
        return Ok((size, None));
    }
    let Some(number) = as_whole_int(page).filter(|&n| n >= 1) else {
        return Ok((
            Bound {
                value: size.value,
                requested: size.requested,
                ceiling,
                error: Some(format!(
                    "Error: page must be a whole number from 1 upwards, not {page}. Nothing was fetched."
                )),
            },
            None,
        ));
    };
    let offset = (number as usize)
        .saturating_sub(1)
        .saturating_mul(size.value);
    if offset > MAX_PAGE_OFFSET {
        return Ok((
            Bound {
                value: size.value,
                requested: size.requested,
                ceiling,
                error: Some(format!(
                    "Error: page {number} at page_size {} starts {offset} records in, past the {MAX_PAGE_OFFSET}-record paging limit. Nothing was fetched.",
                    size.value
                )),
            },
            None,
        ));
    }
    Ok((size, Some(offset)))
}

pub fn bounded_number(value: &Value, name: &str) -> Result<Span> {
    let Some(&(_, minimum, maximum)) = TIMING_BOUNDS.iter().find(|(k, _, _)| *k == name) else {
        bail!("no timing bounds are declared for '{name}'");
    };
    let Some(number) = as_finite_number(value) else {
        return Ok(Span {
            value: minimum,
            error: Some(format!(
                "Error: {name} must be a finite number, not {value}. Infinity and NaN are refused. Nothing was started."
            )),
        });
    };
    if number < minimum || number > maximum {
        return Ok(Span {
            value: minimum,
            error: Some(format!(
                "Error: {name} must be between {minimum} and {maximum}; {number} was given. Nothing was started."
            )),
        });
    }
    Ok(Span {
        value: number,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_bounded_page_and_number_match_contracts() {
        let clamped = bounded(&json!(500), 100, "limit").unwrap();
        assert!(clamped.clamped());
        assert_eq!(clamped.value, 100);
        assert!(clamped.metadata()["limit_note"].as_str().is_some());
        assert!(bounded(&json!(0), 100, "limit").unwrap().error.is_some());
        assert!(bounded(&json!(true), 100, "limit").unwrap().error.is_some());
        assert!(bounded(&json!(1), 0, "limit").is_err());

        let (sz, off) = bounded_page(&json!(2), &json!(20), 100).unwrap();
        assert!(sz.error.is_none());
        assert_eq!(off, Some(20));
        let (deep, none_off) = bounded_page(&json!(200), &json!(50), 100).unwrap();
        assert!(deep.error.as_deref().unwrap().contains("paging limit"));
        assert_eq!(none_off, None);
        let (bad_page, _) = bounded_page(&json!(0), &json!(10), 100).unwrap();
        assert!(bad_page.error.is_some());

        let ok = bounded_number(&json!(5.5), "timeout").unwrap();
        assert!(ok.error.is_none());
        assert_eq!(ok.value, 5.5);
        assert!(
            bounded_number(&json!(0.0), "timeout")
                .unwrap()
                .error
                .is_some()
        );
        assert!(
            bounded_number(&json!("NaN"), "timeout")
                .unwrap()
                .error
                .is_some()
        );
        assert!(
            bounded_number(&json!(0), "lock_grace_seconds")
                .unwrap()
                .error
                .is_none()
        );
        assert!(bounded_number(&json!(1), "unknown_param").is_err());
    }
}
