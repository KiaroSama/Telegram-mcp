use serde_json::Value;
use telegram_mcp::tl::Schema;

pub fn decode_object(kind: &str, fields: &Value) -> Value {
    let schema = Schema::global();
    schema
        .decode(kind, &schema.encode_object(fields).unwrap())
        .unwrap()
}
