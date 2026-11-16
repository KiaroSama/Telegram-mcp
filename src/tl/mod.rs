//! Bounded JSON bridge for the exact TL schema used by the pinned grammers.
//! grammers handles MTProto encryption, RPC errors, connections and DC migration.
//! This bridge supplies less common API methods without a second Telegram client.
mod decode;
mod encode;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock};

pub const LAYER: i32 = 233;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_VECTOR: usize = 100_000;
pub const MAX_DEPTH: usize = 64;
#[derive(Deserialize)]
struct Document {
    layer: i32,
    definitions: Vec<Definition>,
}
#[derive(Deserialize, Debug)]
pub struct Definition {
    pub name: String,
    pub id: u32,
    pub fields: Vec<Field>,
    pub result: String,
    pub category: String,
}
#[derive(Deserialize, Debug)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}
pub struct Schema {
    definitions: Vec<Definition>,
    names: HashMap<String, usize>,
    ids: HashMap<u32, usize>,
}
impl Schema {
    pub fn global() -> &'static Self {
        static SCHEMA: OnceLock<Schema> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            let doc: Document =
                serde_json::from_str(include_str!("../../resources/tl-schema.json"))
                    .expect("bundled schema must be valid");
            assert_eq!(doc.layer, LAYER);
            assert_eq!(
                grammers_tl_types::LAYER,
                LAYER,
                "schema and transport layer disagree"
            );
            let names = doc
                .definitions
                .iter()
                .enumerate()
                .map(|(i, d)| (d.name.clone(), i))
                .collect();
            let ids = doc
                .definitions
                .iter()
                .enumerate()
                .map(|(i, d)| (d.id, i))
                .collect();
            Self {
                definitions: doc.definitions,
                names,
                ids,
            }
        })
    }
    pub fn definition(&self, name: &str) -> Result<&Definition> {
        self.names
            .get(name)
            .map(|i| &self.definitions[*i])
            .context("Unknown Telegram API method or constructor")
    }
    pub fn encode_call(&self, method: &str, params: &Value) -> Result<Vec<u8>> {
        let def = self.definition(method)?;
        if def.category != "functions" {
            bail!("A constructor cannot be invoked as a method");
        }
        let mut bytes = Vec::new();
        self.encode_definition(def, params, &mut bytes, 0)?;
        if bytes.len() > MAX_BYTES {
            bail!("Telegram request exceeds byte budget");
        }
        Ok(bytes)
    }
    pub fn decode_result(&self, method: &str, bytes: &[u8]) -> Result<Value> {
        let result = &self.definition(method)?.result;
        self.decode(result, bytes)
    }
    pub fn decode_response(&self, method: &str, params: &Value, bytes: &[u8]) -> Result<Value> {
        let mut method = method;
        let mut params = params;
        for _ in 0..MAX_DEPTH {
            if self.definition(method)?.result != "X" {
                return self.decode_result(method, bytes);
            }
            params = params
                .get("query")
                .context("A generic Telegram request has no nested query")?;
            method = params
                .get("_")
                .and_then(Value::as_str)
                .context("A nested Telegram request has no method name")?;
            if self.definition(method)?.category != "functions" {
                bail!("A generic query must name an API method");
            }
        }
        bail!("Generic Telegram query nesting exceeds limit")
    }
    pub fn decode(&self, kind: &str, bytes: &[u8]) -> Result<Value> {
        if bytes.len() > MAX_BYTES {
            bail!("Telegram response exceeds byte budget");
        }
        let mut cursor = decode::Reader::new(bytes);
        let result = self.decode_value(kind, &mut cursor, 0)?;
        if cursor.remaining() != 0 {
            bail!("Trailing bytes after Telegram response");
        }
        Ok(result)
    }
}
fn optional(kind: &str) -> Option<(&str, u32, &str)> {
    let (condition, inner) = kind.split_once('?')?;
    let (name, bit) = condition.split_once('.')?;
    let bit: u32 = bit.parse().ok()?;
    if bit >= 32 {
        return None;
    }
    Some((name, bit, inner))
}
fn vector(kind: &str) -> Option<&str> {
    kind.strip_prefix("Vector<")
        .or_else(|| kind.strip_prefix("vector<"))
        .and_then(|v| v.strip_suffix('>'))
}
