fn schema(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn basic_schema() -> serde_json::Map<String, Value> {
    schema(
        json!({"type":"object","properties":{"name":{"type":"string"},"limit":{"type":"integer","default":20}},"required":["name"],"additionalProperties":false}),
    )
}

#[test]
fn all_catalogue_tools_have_native_implementations() {
    let tools = catalogue().unwrap();
    let actual: std::collections::BTreeSet<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(actual.len(), tools.len());
    assert!(actual.contains("send_message"));
    assert!(actual.contains("get_history"));
    assert!(actual.contains("get_poll_results"));
    // All 34 secret-chat tools are now integrated natively alongside cloud tools.
    for secret_tool in [
        "create_secret_chat",
        "send_secret_message",
        "send_secret_media",
        "delete_secret_chat",
    ] {
        assert!(actual.contains(secret_tool));
    }
    for tool in tools {
        assert!(
            tool.annotations.is_some(),
            "{} is missing capability annotations",
            tool.name
        );
        assert!(
            tool.input_schema.get("properties").is_some(),
            "{} has no argument schema",
            tool.name
        );
    }
}
#[test]
fn schema_defaults_are_applied_before_dispatch() {
    let mut args = json!({"name":"Test"});
    validate_arguments(&mut args, &basic_schema()).unwrap();
    assert_eq!(args["limit"], 20);
}
#[test]
fn schema_explicit_values_are_not_overwritten_by_defaults() {
    let mut args = json!({"name":"Test","limit":7});
    validate_arguments(&mut args, &basic_schema()).unwrap();
    assert_eq!(args["limit"], 7);
}
#[test]
fn schema_requires_object_arguments() {
    let mut args = json!([]);
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_missing_required_argument() {
    let mut args = json!({});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_unknown_arguments() {
    let mut args = json!({"name":"Test","typo":true});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_booleans_as_integers() {
    let mut args = json!({"name":"Test","limit":true});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_string_for_strict_integer_field() {
    let mut args = json!({"name":"Test","limit":"20"});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_accepts_explicit_optional_null() {
    let s = schema(
        json!({"properties":{"account":{"anyOf":[{"type":"string"},{"type":"null"}]}},"additionalProperties":false}),
    );
    let mut args = json!({"account":null});
    validate_arguments(&mut args, &s).unwrap();
}
#[test]
fn schema_anyof_rejects_nonmatching_type() {
    let s = schema(
        json!({"properties":{"id":{"anyOf":[{"type":"string"},{"type":"integer"}]}},"additionalProperties":false}),
    );
    let mut args = json!({"id":false});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn schema_numeric_bounds_are_enforced() {
    let s = schema(json!({"properties":{"limit":{"type":"integer","minimum":1,"maximum":200}}}));
    for n in [0, 201] {
        let mut args = json!({"limit":n});
        assert!(validate_arguments(&mut args, &s).is_err());
    }
    let mut args = json!({"limit":200});
    validate_arguments(&mut args, &s).unwrap();
}
#[test]
fn schema_array_items_are_checked_recursively() {
    let s = schema(json!({"properties":{"ids":{"type":"array","items":{"type":"integer"}}}}));
    let mut args = json!({"ids":[1,"2"]});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn schema_rejects_oversized_argument_arrays() {
    let s = schema(json!({"properties":{"ids":{"type":"array"}}}));
    let mut args = json!({"ids":vec![0;100001]});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn content_preserves_image_block_payload_and_metadata() {
    let result = content(
        json!({"content":[{"type":"image","data":"AAECAw==","mimeType":"image/png"}],"metadata":{"source":"native"}}),
    );
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["content"][0]["type"], "image");
    assert_eq!(value["content"][0]["data"], "AAECAw==");
    assert_eq!(value["content"][0]["mimeType"], "image/png");
    assert!(
        value["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["text"].as_str().is_some_and(|s| s.contains("native")))
    );
}
#[test]
fn invalid_native_content_is_returned_as_error() {
    let result = content(json!({"content":[{"type":"unknown"}]}));
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["isError"], true);
}
#[test]
fn scalar_content_is_returned_as_text() {
    let result = content(json!({"ok":true,"id":"9223372036854775807"}));
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["content"][0]["type"], "text");
    assert!(
        value["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("9223372036854775807")
    );
}
#[test]
fn redaction_removes_internal_credentials_recursively() {
    let mut value = json!({"access_hash":"a","rows":[{"id":"7","auth_key":"b","file_reference":[1],"nested":{"api_hash":"c","phone_code_hash":"d","text":"Visible text"}}]});
    redact_internal(&mut value);
    assert!(value.get("access_hash").is_none());
    assert!(value["rows"][0].get("auth_key").is_none());
    assert!(value["rows"][0].get("file_reference").is_none());
    assert!(value["rows"][0]["nested"].get("api_hash").is_none());
    assert_eq!(value["rows"][0]["id"], "7");
    assert_eq!(value["rows"][0]["nested"]["text"], "Visible text");
}

#[test]
fn selector_read_only_uses_annotation_values() {
    let tools = super::catalogue::catalogue_for_mode("read-only").unwrap();
    assert!(tools.iter().any(|t| t.name == "get_history"));
    assert!(
        tools
            .iter()
            .all(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
    );
    assert!(!tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_combined_inclusion_and_exclusion_preserves_read_policy() {
    let tools = super::catalogue::catalogue_for_mode("read-only+send_message-get_me").unwrap();
    assert!(tools.iter().any(|t| t.name == "send_message"));
    assert!(!tools.iter().any(|t| t.name == "get_me"));
    assert!(tools.iter().all(|t| t.name == "send_message"
        || t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)));
}
#[test]
fn selector_exclusion_wins_over_explicit_inclusion() {
    let tools =
        super::catalogue::catalogue_for_mode("read-only+send_message-send_message").unwrap();
    assert!(!tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_trims_outer_whitespace_and_normalizes_case() {
    let tools = super::catalogue::catalogue_for_mode("  READ-ONLY+SEND_MESSAGE-GET_ME  ").unwrap();
    assert!(tools.iter().any(|t| t.name == "send_message"));
    assert!(!tools.iter().any(|t| t.name == "get_me"));
}
#[test]
fn selector_all_mode_can_exclude_writes() {
    let tools = super::catalogue::catalogue_for_mode("all-delete_message").unwrap();
    assert!(!tools.iter().any(|t| t.name == "delete_message"));
    assert!(tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_all_mode_rejects_additive_syntax() {
    assert!(super::catalogue::catalogue_for_mode("all+send_message").is_err());
}
#[test]
fn selector_unknown_names_are_not_silently_ignored() {
    for mode in [
        "read-only+unknown_tool",
        "all-unknown_tool",
        "read-only+send_message-unknown_tool",
    ] {
        assert!(
            super::catalogue::catalogue_for_mode(mode).is_err(),
            "Accepted {mode}"
        );
    }
}
#[test]
fn selector_malformed_or_empty_lists_are_rejected() {
    for mode in [
        "read-only+",
        "all-",
        "read-only+send_message,",
        "read-only+send_message,,get_me",
        "read-only+send message",
        "read-only+send_message-",
        "read-only-get_me+send_message",
        "allover",
        "",
    ] {
        assert!(
            super::catalogue::catalogue_for_mode(mode).is_err(),
            "Accepted {mode}"
        );
    }
}
#[test]
fn selector_duplicate_inclusions_do_not_duplicate_tool_descriptors() {
    let tools =
        super::catalogue::catalogue_for_mode("read-only+send_message,send_message").unwrap();
    assert_eq!(tools.iter().filter(|t| t.name == "send_message").count(), 1);
}
#[test]
fn all_nullable_defaults_match_declared_argument_types() {
    for tool in super::catalogue::catalogue_for_mode("all").unwrap() {
        for (key, spec) in tool.input_schema["properties"].as_object().unwrap() {
            if spec.get("default") == Some(&Value::Null) {
                let s = schema(json!({"properties":{key:spec},"additionalProperties":false}));
                let mut args = json!({key:null});
                assert!(
                    validate_arguments(&mut args, &s).is_ok(),
                    "{} default null contradicts argument type for {key}",
                    tool.name
                );
            }
        }
    }
}
