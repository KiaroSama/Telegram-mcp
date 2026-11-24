use super::*;
use std::collections::BTreeSet;

pub fn catalogue() -> Result<Vec<Tool>> {
    let mode = std::env::var("TELEGRAM_EXPOSED_TOOLS").unwrap_or_else(|_| "all".into());
    catalogue_for_mode(&mode)
}
pub fn catalogue_for_mode(mode: &str) -> Result<Vec<Tool>> {
    let descriptors: Vec<Value> =
        serde_json::from_str(include_str!("../../resources/tool-catalogue.json"))?;
    let implemented = crate::tools::identity::TOOL_NAMES
        .iter()
        .chain(crate::tools::messaging::TOOL_NAMES)
        .chain(crate::tools::advanced::TOOL_NAMES)
        .chain(crate::tools::local::TOOL_NAMES)
        .chain(crate::tools::secret::TOOL_NAMES)
        .chain(crate::tools::visual::TOOL_NAMES)
        .chain(crate::tools::EXTRA_TOOL_NAMES)
        .copied()
        .collect::<BTreeSet<_>>();
    let mut tools = Vec::new();
    let mut seen = BTreeSet::new();
    for descriptor in descriptors {
        let tool: Tool = serde_json::from_value(descriptor)?;
        if !implemented.contains(tool.name.as_ref()) {
            bail!("Catalogue tool has no native route: {}", tool.name);
        }
        if !seen.insert(tool.name.to_string()) {
            bail!("Duplicate tool in catalogue");
        }
        tools.push(tool);
    }
    let mode = mode.trim().to_ascii_lowercase();
    let (base, suffix) = if let Some(suffix) = mode.strip_prefix("read-only") {
        ("read-only", suffix)
    } else if let Some(suffix) = mode.strip_prefix("all") {
        ("all", suffix)
    } else {
        bail!("TELEGRAM_EXPOSED_TOOLS must start with all or read-only");
    };
    let (allow, exclude) = if let Some(suffix) = suffix.strip_prefix('+') {
        suffix.split_once('-').unwrap_or((suffix, ""))
    } else if let Some(suffix) = suffix.strip_prefix('-') {
        ("", suffix)
    } else if suffix.is_empty() {
        ("", "")
    } else {
        bail!("Invalid exposed-tool mode suffix");
    };
    if suffix.starts_with('+') && allow.is_empty() || suffix.contains('-') && exclude.is_empty() {
        bail!("Exposed-tool allow and exclude lists must not be empty");
    }
    for list in [allow, exclude] {
        if !list.is_empty() && list.split(',').any(|name| name.trim().is_empty()) {
            bail!("Exposed-tool lists contain an empty tool name");
        }
    }
    let allow = names(allow);
    let exclude = names(exclude);
    for name in allow.iter().chain(&exclude) {
        if !seen.contains(name) {
            bail!("Unknown exposed tool name: {name}");
        }
    }
    match base {
        "all" => {
            if !allow.is_empty() {
                bail!("Additional tool names are only valid for read-only mode");
            }
        }
        "read-only" => tools.retain(|t| {
            t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)
                || allow.contains(t.name.as_ref())
        }),
        _ => bail!(
            "TELEGRAM_EXPOSED_TOOLS must be all or read-only, with optional +allow and -exclude tool lists"
        ),
    }
    tools.retain(|tool| !exclude.contains(tool.name.as_ref()));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tools)
}
fn names(text: &str) -> BTreeSet<String> {
    text.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}
pub fn validate_arguments(args: &mut Value, schema: &serde_json::Map<String, Value>) -> Result<()> {
    let object = args
        .as_object_mut()
        .context("Tool arguments must be an object")?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .context("Tool schema has no properties")?;
    if schema.get("additionalProperties") == Some(&json!(false)) {
        for key in object.keys() {
            if !properties.contains_key(key) {
                bail!("Unknown tool argument: {key}");
            }
        }
    }
    for (key, spec) in properties {
        if !object.contains_key(key)
            && let Some(default) = spec.get("default")
        {
            object.insert(key.clone(), default.clone());
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required {
            let key = key.as_str().context("Invalid required-property schema")?;
            if !object.contains_key(key) {
                bail!("Required tool argument is missing: {key}");
            }
        }
    }
    for (key, value) in object {
        if let Some(spec) = properties.get(key) {
            validate(value, spec, key)?;
        }
    }
    Ok(())
}
fn validate(value: &Value, spec: &Value, key: &str) -> Result<()> {
    if let Some(choices) = spec.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        bail!("Invalid argument value for {key}");
    }
    if let Some(choices) = spec.get("anyOf").and_then(Value::as_array) {
        if !choices
            .iter()
            .any(|choice| validate(value, choice, key).is_ok())
        {
            bail!("Invalid argument type for {key}");
        }
        return Ok(());
    }
    if let Some(kind) = spec.get("type") {
        let matches = |kind: &str| match kind {
            "null" => value.is_null(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            _ => false,
        };
        let valid = kind.as_str().map(matches).unwrap_or_else(|| {
            kind.as_array()
                .is_some_and(|kinds| kinds.iter().filter_map(Value::as_str).any(matches))
        });
        if !valid {
            bail!("Invalid argument type for {key}");
        }
    }
    if let Some(n) = value.as_f64()
        && (spec
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| n < min)
            || spec
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| n > max))
    {
        bail!("Argument {key} is outside its permitted range");
    }
    if let Some(values) = value.as_array() {
        if values.len() > 100_000 {
            bail!("Argument {key} has too many items");
        }
        if let Some(item) = spec.get("items") {
            for value in values {
                validate(value, item, key)?;
            }
        }
    }
    if let Some(text) = value.as_str()
        && text.len() > 16 * 1024 * 1024
    {
        bail!("Argument {key} exceeds its byte budget");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_mode_exposes_only_read_only_secret_tools() {
        let tools = catalogue_for_mode("read-only").unwrap();
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .filter(|name| crate::tools::secret::TOOL_NAMES.contains(name))
            .collect();
        assert_eq!(
            names,
            [
                "get_secret_backend_status",
                "get_secret_pause_status",
                "list_secret_chats",
                "list_secret_deliveries",
                "read_secret_messages",
                "search_secret_messages",
                "secret_chat_status",
            ]
        );
    }
}
