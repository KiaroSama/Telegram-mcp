//! Sanitization utilities layered on [`crate::text_fidelity`].
//!
//! Projects untrusted Telegram text, display names, paired entities, and nested
//! JSON values consistently while preserving typed numbers, booleans, nulls,
//! IDs, and trusted metadata. Composes with an optional [`SealRedactor`] both
//! before and after projection so neither ordering nor invisible-character
//! stripping can reveal a sealed approval code.

use serde_json::{Map, Value};

use crate::text_fidelity::{
    DEFAULT_MAX_NAME_CHARS, DEFAULT_MAX_TEXT_CHARS, EMPTY_PLACEHOLDER, ProjectionOptions,
    RawTextEntity, TextFidelityResult, project_text,
};

/// Hook implemented by `safeguard::sealed` (or test doubles) to redact sealed
/// approval prompts and codes during text/value sanitization.
pub trait SealRedactor {
    /// Returns redacted replacement text when `text` matches sealed material,
    /// or `None` when nothing requires redaction.
    fn redact_text(&self, text: &str) -> Option<String>;
}

/// Closure adapter for [`SealRedactor`].
pub struct FnRedactor<F>(pub F);

impl<F> SealRedactor for FnRedactor<F>
where
    F: Fn(&str) -> Option<String>,
{
    fn redact_text(&self, text: &str) -> Option<String> {
        (self.0)(text)
    }
}

/// Options for recursive [`serde_json::Value`] sanitization.
#[derive(Clone, Copy)]
pub struct SanitizeValueOptions<'a> {
    pub max_length: usize,
    pub trusted_keys: &'a [&'a str],
    pub single_line_keys: &'a [&'a str],
    pub redactor: Option<&'a dyn SealRedactor>,
}

impl Default for SanitizeValueOptions<'_> {
    fn default() -> Self {
        Self {
            max_length: DEFAULT_MAX_TEXT_CHARS,
            trusted_keys: &[],
            single_line_keys: &[],
            redactor: None,
        }
    }
}

fn sanitize_name_options(max_length: usize) -> ProjectionOptions {
    ProjectionOptions {
        normalize_breaks: false,
        collapse_newlines: false,
        single_line: true,
        trim: true,
        empty_placeholder: true,
        strip_ascii_joiners: true,
        max_chars: Some(max_length),
        truncation_marker: crate::text_fidelity::TRUNCATED_MARKER,
    }
}

fn redact_metadata_value(value: Value, redactor: &dyn SealRedactor, max_length: usize) -> Value {
    match value {
        Value::String(s) => {
            if let Some(redacted) = redactor.redact_text(&s) {
                Value::String(
                    project_text(
                        Some(&redacted),
                        &[],
                        ProjectionOptions::sanitized(max_length),
                    )
                    .text,
                )
            } else {
                Value::String(s)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact_metadata_value(item, redactor, max_length))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, redact_metadata_value(v, redactor, max_length)))
                .collect(),
        ),
        other => other,
    }
}

fn project_with_redactor(
    text: Option<&str>,
    entities: &[RawTextEntity],
    options: ProjectionOptions,
    redactor: Option<&dyn SealRedactor>,
) -> TextFidelityResult {
    let raw = text.unwrap_or("");
    let mut pre_redacted = false;
    let working = if let Some(r) = redactor
        && let Some(redacted) = r.redact_text(raw)
    {
        pre_redacted = true;
        redacted
    } else {
        raw.to_string()
    };

    let active_entities: &[RawTextEntity] = if pre_redacted { &[] } else { entities };
    let mut result = project_text(Some(&working), active_entities, options);
    if pre_redacted {
        result.metadata.dropped_entities = entities.len();
    }

    if let Some(r) = redactor {
        if let Some(redacted_after) = r.redact_text(&result.text) {
            let mut safe = project_text(Some(&redacted_after), &[], options);
            safe.metadata.dropped_entities = entities.len();
            return safe;
        }
        let max_len = options.max_chars.unwrap_or(DEFAULT_MAX_TEXT_CHARS);
        for entity in &mut result.entities {
            let updated: Map<String, Value> = std::mem::take(&mut entity.metadata)
                .into_iter()
                .map(|(k, v)| (k, redact_metadata_value(v, r, max_len)))
                .collect();
            entity.metadata = updated;
        }
    }

    result
}

/// Projects paired untrusted text and Telegram UTF-16 entities with optional
/// two-pass seal redaction.
pub fn sanitize_text_and_entities(
    text: Option<&str>,
    entities: &[RawTextEntity],
    max_length: usize,
    redactor: Option<&dyn SealRedactor>,
) -> TextFidelityResult {
    project_with_redactor(
        text,
        entities,
        ProjectionOptions::sanitized(max_length),
        redactor,
    )
}

/// Keep Telegram's wire entity shape while moving spans onto the returned string.
pub(crate) fn project_entity_fields(
    fields: &mut Map<String, Value>,
    text_key: &str,
    entities_key: &str,
    options: ProjectionOptions,
    redactor: Option<&dyn SealRedactor>,
) {
    let Some(text) = fields.get(text_key).and_then(Value::as_str) else {
        return;
    };
    let entities = fields.get(entities_key).and_then(Value::as_array);
    let raw: Vec<_> = entities
        .into_iter()
        .flatten()
        .filter_map(|entity| {
            let mut metadata = entity.as_object()?.clone();
            let offset = metadata.remove("offset")?.as_i64()?;
            let length = metadata.remove("length")?.as_i64()?;
            let kind = metadata
                .get("_")
                .or_else(|| metadata.get("type"))?
                .as_str()?
                .to_owned();
            for field in ["url", "language"] {
                if let Some(s) = metadata.get(field).and_then(Value::as_str) {
                    let clean = crate::text_fidelity::display_name(Some(s), DEFAULT_MAX_TEXT_CHARS);
                    if clean != s {
                        metadata.insert(field.into(), clean.into());
                        metadata.insert(format!("{field}_altered"), true.into());
                    }
                }
            }
            Some(RawTextEntity {
                kind,
                offset,
                length,
                metadata,
            })
        })
        .collect();
    let projected = project_with_redactor(Some(text), &raw, options, redactor);
    if entities.is_some() {
        let kept = projected
            .entities
            .into_iter()
            .map(|e| {
                let mut row = e.metadata;
                row.insert("offset".into(), e.offset.into());
                row.insert("length".into(), e.length.into());
                Value::Object(row)
            })
            .collect();
        fields.insert(entities_key.into(), Value::Array(kept));
    }
    fields.insert(text_key.into(), projected.text.into());
}

/// Sanitizes multi-line user-controlled text content with the 4,096-char cap
/// (or caller-specified `max_length`), returning `[empty]` when no visible text
/// remains.
pub fn sanitize_user_content(text: Option<&str>, max_length: usize) -> String {
    sanitize_user_content_with_redactor(text, max_length, None)
}

/// Sanitizes multi-line user content while composing with an optional seal redactor.
pub fn sanitize_user_content_with_redactor(
    text: Option<&str>,
    max_length: usize,
    redactor: Option<&dyn SealRedactor>,
) -> String {
    project_with_redactor(
        text,
        &[],
        ProjectionOptions::sanitized(max_length),
        redactor,
    )
    .text
}

/// Sanitizes a single-line display name (username, chat title, sender name).
pub fn sanitize_name(text: Option<&str>, max_length: usize) -> String {
    sanitize_name_with_redactor(text, max_length, None)
}

/// Sanitizes a single-line display name with optional seal redaction.
pub fn sanitize_name_with_redactor(
    text: Option<&str>,
    max_length: usize,
    redactor: Option<&dyn SealRedactor>,
) -> String {
    project_with_redactor(text, &[], sanitize_name_options(max_length), redactor).text
}

/// Joins optional `first_name` and `last_name` without ever printing `"None"`.
pub fn full_name(first_name: Option<&str>, last_name: Option<&str>) -> String {
    let parts: Vec<&str> = [first_name, last_name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    parts.join(" ")
}

/// Lossily decodes raw bytes as UTF-8 and applies standard user-content sanitization.
pub fn sanitize_bytes(bytes: &[u8], max_length: usize) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    sanitize_user_content(Some(decoded.as_ref()), max_length)
}

/// Recursively sanitizes all untrusted string values inside a JSON value using
/// default 4,096-char bounding while preserving numbers, booleans, and nulls.
pub fn sanitize_value(value: Value) -> Value {
    sanitize_value_with_options(value, &SanitizeValueOptions::default())
}

/// Recursively sanitizes a JSON value with configurable trusted keys, single-line
/// keys, length cap, and optional seal redaction.
pub fn sanitize_value_with_options(value: Value, options: &SanitizeValueOptions<'_>) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize_user_content_with_redactor(
            Some(&s),
            options.max_length,
            options.redactor,
        )),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| sanitize_value_with_options(item, options))
                .collect(),
        ),
        Value::Object(mut map) => {
            let projected = [
                ("message", "entities"),
                ("text", "entities"),
                ("quote_text", "quote_entities"),
            ]
            .into_iter()
            .find(|(text, spans)| {
                map.get(*text).is_some_and(Value::is_string)
                    && map.get(*spans).is_some_and(Value::is_array)
            });
            if let Some((text, spans)) = projected {
                project_entity_fields(
                    &mut map,
                    text,
                    spans,
                    ProjectionOptions::sanitized(options.max_length),
                    options.redactor,
                );
            }
            let mut out = Map::with_capacity(map.len());
            for (key, val) in map {
                let sanitized = if projected.is_some_and(|(text, _)| text == key) {
                    val
                } else if options.trusted_keys.contains(&key.as_str()) {
                    sanitize_trusted_field(val, options)
                } else if options.single_line_keys.contains(&key.as_str()) {
                    match val {
                        Value::String(s) => Value::String(sanitize_name_with_redactor(
                            Some(&s),
                            options.max_length.min(DEFAULT_MAX_NAME_CHARS),
                            options.redactor,
                        )),
                        other => sanitize_value_with_options(other, options),
                    }
                } else {
                    sanitize_value_with_options(val, options)
                };
                out.insert(key, sanitized);
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn sanitize_trusted_field(value: Value, options: &SanitizeValueOptions<'_>) -> Value {
    match value {
        Value::String(s) => {
            if let Some(r) = options.redactor
                && let Some(redacted) = r.redact_text(&s)
            {
                return Value::String(sanitize_user_content(Some(&redacted), options.max_length));
            }
            Value::String(s)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| sanitize_trusted_field(item, options))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, sanitize_trusted_field(v, options)))
                .collect(),
        ),
        other => other,
    }
}

/// Formats structured tool output as UTF-8 JSON under `{"results": [...], ...metadata}`.
pub fn format_tool_result(
    records: Vec<Value>,
    metadata: Option<Map<String, Value>>,
) -> serde_json::Result<String> {
    let mut payload = Map::new();
    payload.insert("results".to_string(), Value::Array(records));
    if let Some(meta) = metadata {
        for (k, v) in meta {
            payload.insert(k, v);
        }
    }
    serde_json::to_string(&Value::Object(payload))
}

/// Convenience helper for checking whether sanitized text resolved to `[empty]`.
pub fn is_empty_placeholder(text: &str) -> bool {
    text == EMPTY_PLACEHOLDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_fidelity::TRUNCATED_MARKER;
    use serde_json::json;

    const PERSIAN: &str = "\u{0645}\u{06CC}\u{200C}\u{06A9}\u{0646}\u{062F}";
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    const SCOTLAND: &str = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}";

    #[test]
    fn user_content_preserves_visible_unicode_and_strips_hidden_noise() {
        assert_eq!(sanitize_user_content(None, 4096), "[empty]");
        assert_eq!(sanitize_user_content(Some("  \n\t "), 4096), "[empty]");
        assert_eq!(
            sanitize_user_content(Some("Привет мир 你好 🎉"), 4096),
            "Привет мир 你好 🎉"
        );
        assert_eq!(
            sanitize_user_content(Some("line1\nline2\tindented"), 4096),
            "line1\nline2\tindented"
        );
        assert_eq!(sanitize_user_content(Some("a\n\n\n\n\nb"), 4096), "a\n\nb");
        assert_eq!(
            sanitize_user_content(Some("hello\x00world\x07test"), 4096),
            "helloworldtest"
        );
        assert_eq!(
            sanitize_user_content(Some("normal\u{202E}reversed"), 4096),
            "normalreversed"
        );
        assert_eq!(
            sanitize_user_content(Some("hello\u{200B}world\u{200D}test\u{FEFF}end"), 4096),
            "helloworldtestend"
        );
        assert_eq!(
            sanitize_user_content(Some("abc\u{200E} \u{200F}دف"), 4096),
            "abc\u{200E} \u{200F}دف"
        );

        let rich = format!("Team {SCOTLAND} {PERSIAN} {FAMILY}");
        assert_eq!(sanitize_user_content(Some(&rich), 4096), rich);
        assert_eq!(sanitize_name(Some(&rich), 256), rich);
    }

    #[test]
    fn ascii_joiners_and_stray_tags_are_stripped_and_caps_include_marker() {
        for hidden in [
            '\u{200B}', '\u{2060}', '\u{FEFF}', '\u{202E}', '\u{2066}', '\u{200C}', '\u{200D}',
        ] {
            assert_eq!(
                sanitize_name(Some(&format!("ig{hidden}nore")), 256),
                "ignore"
            );
        }
        assert_eq!(sanitize_name(Some("a\u{E0067}\u{E007F}b"), 256), "ab");
        assert_eq!(sanitize_name(Some("John\n\n\nDoe"), 256), "John Doe");

        let capped = sanitize_user_content(Some(&"a".repeat(5000)), 100);
        assert_eq!(capped.chars().count(), 100);
        assert!(capped.ends_with(TRUNCATED_MARKER));

        let exact_cap = sanitize_user_content(Some(&"a".repeat(4096)), 4096);
        assert_eq!(exact_cap.chars().count(), 4096);
        assert!(!exact_cap.ends_with(TRUNCATED_MARKER));

        let over_cap = sanitize_user_content(Some(&"a".repeat(5000)), 4096);
        assert_eq!(over_cap.chars().count(), 4096);
        assert!(over_cap.ends_with(TRUNCATED_MARKER));

        assert_eq!(sanitize_user_content(Some("a:b:c"), 0), TRUNCATED_MARKER);
        assert_eq!(sanitize_name(Some("a:b:c"), 0), TRUNCATED_MARKER);
        assert_eq!(sanitize_user_content(Some("   "), 0), EMPTY_PLACEHOLDER);
    }

    #[test]
    fn full_name_and_bytes_match_baseline_behavior() {
        assert_eq!(
            full_name(Some("GodVerify Payment Bot"), None),
            "GodVerify Payment Bot"
        );
        assert_eq!(full_name(Some("Sara"), Some("K")), "Sara K");
        assert_eq!(full_name(None, None), "");
        assert_eq!(sanitize_bytes(b"ok\x00\xffend", 4096), "ok\u{FFFD}end");
    }

    #[test]
    fn sanitize_value_preserves_types_and_trusted_keys_while_projecting_strings() {
        let input = json!({
            "id": 42,
            "active": true,
            "score": 3.5,
            "empty": null,
            "cursor": "",
            "user": {"name": "John\n\x00Doe", "bio": "hi\u{200B}there\n\n\n\nnow"}
        });
        let opts = SanitizeValueOptions {
            max_length: 4096,
            trusted_keys: &["cursor"],
            single_line_keys: &["name"],
            redactor: None,
        };
        let out = sanitize_value_with_options(input, &opts);
        assert_eq!(out["id"], json!(42));
        assert_eq!(out["active"], json!(true));
        assert_eq!(out["score"], json!(3.5));
        assert_eq!(out["empty"], Value::Null);
        assert_eq!(out["cursor"], json!(""));
        assert_eq!(out["user"]["name"], json!("John Doe"));
        assert_eq!(out["user"]["bio"], json!("hithere\n\nnow"));
    }

    #[test]
    fn seal_redaction_blocks_direct_and_obfuscated_approval_codes_with_entities() {
        let secret_code = "SEAL-7741-CODE";
        let redactor = FnRedactor(|s: &str| {
            s.contains(secret_code)
                .then(|| "[redacted approval prompt]".to_string())
        });

        // Obfuscated with zero-width space AND Latin-flanked ZWNJ: post-pass catches reassembled code!
        let obfuscated = "Approve: SEAL\u{200B}-77\u{200C}41-CODE now";
        let entities = vec![RawTextEntity {
            kind: "code".into(),
            offset: 9,
            length: 16,
            metadata: [("raw".into(), json!(secret_code))].into_iter().collect(),
        }];
        let res = sanitize_text_and_entities(Some(obfuscated), &entities, 4096, Some(&redactor));
        assert_eq!(res.text, "[redacted approval prompt]");
        assert!(!res.text.contains(secret_code));
        assert!(res.entities.is_empty());
        assert_eq!(res.metadata.dropped_entities, 1);

        // Even entity metadata carrying a sealed code is redacted
        let clean_text = "normal message";
        let leak_entities = vec![RawTextEntity {
            kind: "text_url".into(),
            offset: 0,
            length: 6,
            metadata: [(
                "url".into(),
                json!("https://example.test/?c=SEAL-7741-CODE"),
            )]
            .into_iter()
            .collect(),
        }];
        let res2 =
            sanitize_text_and_entities(Some(clean_text), &leak_entities, 4096, Some(&redactor));
        assert_eq!(res2.text, "normal message");
        assert_eq!(res2.entities.len(), 1);
        assert_eq!(
            res2.entities[0].metadata["url"],
            json!("[redacted approval prompt]")
        );

        // Even a trusted key in sanitize_value cannot bypass seal redaction
        let val = json!({"status": "SEAL-7741-CODE", "text": obfuscated});
        let opts = SanitizeValueOptions {
            trusted_keys: &["status"],
            redactor: Some(&redactor),
            ..SanitizeValueOptions::default()
        };
        let sanitized = sanitize_value_with_options(val, &opts);
        assert_eq!(sanitized["status"], json!("[redacted approval prompt]"));
        assert_eq!(sanitized["text"], json!("[redacted approval prompt]"));
    }

    #[test]
    fn format_tool_result_preserves_utf8_and_merges_metadata() {
        let mut meta = Map::new();
        meta.insert("total".into(), json!(1));
        let raw = format_tool_result(vec![json!({"text": "Привет"})], Some(meta))
            .expect("json serialization");
        assert!(raw.contains("Привет"));
        let parsed: Value = serde_json::from_str(&raw).expect("valid json");
        assert_eq!(parsed["total"], json!(1));
        assert_eq!(parsed["results"][0]["text"], json!("Привет"));
    }
}
