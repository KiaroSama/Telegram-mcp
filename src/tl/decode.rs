use super::*;
use serde_json::{Map, json};
pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(n)
            .context("Telegram byte offset overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .context("Truncated Telegram response")?;
        self.position = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let head = self.take(1)?[0];
        let (length, header) = match head {
            0..=253 => (head as usize, 1),
            254 => {
                let b = self.take(3)?;
                (
                    (b[0] as usize) | ((b[1] as usize) << 8) | ((b[2] as usize) << 16),
                    4,
                )
            }
            _ => bail!("Invalid Telegram byte-string marker"),
        };
        let value = self.take(length)?.to_vec();
        let padding = (4 - (header + length) % 4) % 4;
        if self.take(padding)?.iter().any(|v| *v != 0) {
            bail!("Invalid Telegram byte-string padding");
        }
        Ok(value)
    }
}
impl Schema {
    pub(super) fn decode_value(
        &self,
        kind: &str,
        cursor: &mut Reader<'_>,
        depth: usize,
    ) -> Result<Value> {
        if depth > MAX_DEPTH {
            bail!("Telegram response nesting exceeds limit");
        }
        if let Some(inner) = vector(kind) {
            if kind.starts_with("Vector") && cursor.u32()? != 0x1cb5c415 {
                bail!("Invalid Telegram vector constructor");
            }
            let length = cursor.u32()? as usize;
            if length > MAX_VECTOR {
                bail!("Telegram vector exceeds limit");
            }
            let mut values = Vec::with_capacity(length.min(1024));
            for _ in 0..length {
                values.push(self.decode_value(inner, cursor, depth + 1)?);
            }
            return Ok(Value::Array(values));
        }
        Ok(match kind {
            "int" => json!(cursor.u32()? as i32),
            "long" => json!(i64::from_le_bytes(cursor.take(8)?.try_into()?).to_string()),
            "double" => {
                let value = f64::from_le_bytes(cursor.take(8)?.try_into()?);
                if !value.is_finite() {
                    bail!("Non-finite Telegram number");
                }
                json!(value)
            }
            "Bool" => match cursor.u32()? {
                0x997275b5 => json!(true),
                0xbc799737 => json!(false),
                _ => bail!("Invalid Telegram boolean constructor"),
            },
            "true" => json!(true),
            "string" => json!(
                String::from_utf8(cursor.bytes()?).context("Telegram text is not valid UTF-8")?
            ),
            "bytes" => json!(cursor.bytes()?),
            "int128" => json!(cursor.take(16)?),
            "int256" => json!(cursor.take(32)?),
            _ => {
                let id = cursor.u32()?;
                let def = self
                    .ids
                    .get(&id)
                    .map(|i| &self.definitions[*i])
                    .with_context(|| {
                        format!("Unknown Telegram constructor {id:08x} for pinned layer {LAYER}")
                    })?;
                if kind != "X" && kind != "Object" && def.result != kind {
                    bail!("Unexpected Telegram constructor for {kind}");
                }
                let mut object = Map::new();
                object.insert("_".into(), json!(def.name));
                let mut flags: HashMap<&str, u32> = HashMap::new();
                for field in &def.fields {
                    if field.kind == "#" {
                        flags.insert(&field.name, cursor.u32()?);
                        continue;
                    }
                    let inner = if let Some((flag, bit, inner)) = optional(&field.kind) {
                        let present = flags
                            .get(flag)
                            .copied()
                            .context("Schema flag referenced before definition")?
                            & (1 << bit)
                            != 0;
                        if inner == "true" {
                            object.insert(field.name.clone(), json!(present));
                            continue;
                        }
                        if !present {
                            object.insert(field.name.clone(), Value::Null);
                            continue;
                        }
                        inner
                    } else {
                        &field.kind
                    };
                    object.insert(
                        field.name.clone(),
                        self.decode_value(inner, cursor, depth + 1)?,
                    );
                }
                Value::Object(object)
            }
        })
    }
}
