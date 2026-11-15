//! Generic request wrappers must preserve the actual nested return type.
use grammers_tl_types::{self as tl, Serializable};
use serde_json::{Value, json};
use telegram_mcp::tl::Schema;

fn wrapped(query: Value) -> Value {
    json!({"takeout_id":"123456789","query":query})
}

#[test]
fn takeout_boolean_response_remains_a_boolean_including_false() {
    let params = wrapped(json!({"_":"account.finishTakeoutSession","success":true}));
    Schema::global()
        .encode_call("invokeWithTakeout", &params)
        .unwrap();
    for value in [true, false] {
        assert_eq!(
            Schema::global()
                .decode_response("invokeWithTakeout", &params, &value.to_bytes())
                .unwrap(),
            json!(value)
        );
    }
}

#[test]
fn takeout_vector_response_preserves_lossless_longs() {
    let params = wrapped(json!({"_":"messages.receivedQueue","max_qts":0}));
    Schema::global()
        .encode_call("invokeWithTakeout", &params)
        .unwrap();
    let bytes = vec![i64::MIN, 0, i64::MAX].to_bytes();
    assert_eq!(
        Schema::global()
            .decode_response("invokeWithTakeout", &params, &bytes)
            .unwrap(),
        json!(["-9223372036854775808", "0", "9223372036854775807"])
    );
}

#[test]
fn takeout_typed_vectors_keep_objects_and_empty_results() {
    let params = wrapped(json!({"_":"users.getUsers","id":[]}));
    Schema::global()
        .encode_call("invokeWithTakeout", &params)
        .unwrap();
    let values = vec![tl::enums::User::Empty(tl::types::UserEmpty { id: 123 })];
    assert_eq!(
        Schema::global()
            .decode_response("invokeWithTakeout", &params, &values.to_bytes())
            .unwrap(),
        json!([{"_":"userEmpty","id":"123"}])
    );
    assert_eq!(
        Schema::global()
            .decode_response(
                "invokeWithTakeout",
                &params,
                &Vec::<tl::enums::User>::new().to_bytes()
            )
            .unwrap(),
        json!([])
    );
}

#[test]
fn nested_generic_wrappers_infer_the_innermost_return_type() {
    let params = wrapped(
        json!({"_":"invokeWithoutUpdates","query":{"_":"invokeAfterMsgs","msg_ids":["9","10"],"query":{"_":"account.finishTakeoutSession"}}}),
    );
    Schema::global()
        .encode_call("invokeWithTakeout", &params)
        .unwrap();
    assert_eq!(
        Schema::global()
            .decode_response("invokeWithTakeout", &params, &false.to_bytes())
            .unwrap(),
        json!(false)
    );
}

#[test]
fn direct_results_and_wrapped_constructors_use_the_same_schema_constraints() {
    let params = wrapped(json!({"_":"users.getUsers","id":[]}));
    assert!(
        Schema::global()
            .decode_response("invokeWithTakeout", &params, &true.to_bytes())
            .is_err()
    );
    let mut bytes = false.to_bytes();
    bytes.extend(0u32.to_le_bytes());
    assert!(
        Schema::global()
            .decode_response(
                "invokeWithTakeout",
                &wrapped(json!({"_":"account.finishTakeoutSession"})),
                &bytes
            )
            .is_err()
    );
    assert_eq!(
        Schema::global()
            .decode_response(
                "account.finishTakeoutSession",
                &json!({}),
                &false.to_bytes()
            )
            .unwrap(),
        json!(false)
    );
}

#[test]
fn malformed_generic_requests_never_decode_untyped_constructor_data() {
    for params in [
        json!({}),
        json!({"query":null}),
        json!({"query":{}}),
        json!({"query":{"_":"boolTrue"}}),
        json!({"query":{"_":"missing.method"}}),
    ] {
        assert!(
            Schema::global()
                .decode_response("invokeWithTakeout", &params, &true.to_bytes())
                .is_err()
        );
    }
}

#[test]
fn generic_query_nesting_is_bounded_without_recursion_or_panic() {
    let mut query = json!({"_":"account.finishTakeoutSession"});
    for _ in 0..100 {
        query = json!({"_":"invokeWithoutUpdates","query":query});
    }
    let error = Schema::global()
        .decode_response("invokeWithTakeout", &wrapped(query), &true.to_bytes())
        .unwrap_err()
        .to_string();
    assert!(error.contains("nesting"));
}
