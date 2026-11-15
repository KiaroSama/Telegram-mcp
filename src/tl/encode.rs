use super::*;
use base64::Engine;

fn number(value: &Value) -> Result<i64> {
    if let Some(n) = value.as_i64() {
        return Ok(n);
    }
    value
        .as_str()
        .context("Expected an integer or decimal integer string")?
        .parse()
        .context("Invalid integer")
}
impl Schema {
    pub(super) fn encode_definition(
        &self,
        def: &Definition,
        params: &Value,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            bail!("Telegram object nesting exceeds limit");
        }
        let params = params
            .as_object()
            .context("Telegram method parameters must be an object")?;
        for key in params.keys() {
            if key != "_" && !def.fields.iter().any(|f| &f.name == key && f.kind != "#") {
                bail!("Unexpected parameter {key} for {}", def.name);
            }
        }
        out.extend(def.id.to_le_bytes());
        let mut flags: HashMap<&str, u32> = HashMap::new();
        for field in &def.fields {
            if let Some((name, bit, inner)) = optional(&field.kind)
                && let Some(value) = params.get(&field.name).filter(|v| !v.is_null())
                && (inner != "true" || value.as_bool().context("Flag value must be a boolean")?)
            {
                *flags.entry(name).or_default() |= 1 << bit;
            }
        }
        for field in &def.fields {
            if field.kind == "#" {
                out.extend(
                    flags
                        .get(field.name.as_str())
                        .copied()
                        .unwrap_or(0)
                        .to_le_bytes(),
                );
                continue;
            }
            let kind = if let Some((name, bit, inner)) = optional(&field.kind) {
                if flags.get(name).copied().unwrap_or(0) & (1 << bit) == 0 {
                    continue;
                }
                if inner == "true" {
                    continue;
                }
                inner
            } else {
                &field.kind
            };
            let value = params
                .get(&field.name)
                .filter(|v| !v.is_null())
                .with_context(|| format!("{} requires {}", def.name, field.name))?;
            self.encode_value(kind, value, out, depth + 1)?;
        }
        Ok(())
    }
    fn encode_value(
        &self,
        kind: &str,
        value: &Value,
        out: &mut Vec<u8>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH || out.len() > MAX_BYTES {
            bail!("Telegram encoding budget exceeded");
        }
        if let Some(inner) = vector(kind) {
            let values = value.as_array().context("Expected an array")?;
            if values.len() > MAX_VECTOR {
                bail!("Telegram array exceeds limit");
            }
            if kind.starts_with("Vector") {
                out.extend(0x1cb5c415u32.to_le_bytes());
            }
            out.extend((values.len() as i32).to_le_bytes());
            for value in values {
                self.encode_value(inner, value, out, depth + 1)?;
            }
            return Ok(());
        }
        match kind {
            "int" => out.extend(
                i32::try_from(number(value)?)
                    .context("Integer exceeds signed 32-bit range")?
                    .to_le_bytes(),
            ),
            "long" => out.extend(number(value)?.to_le_bytes()),
            "double" => out.extend(
                value
                    .as_f64()
                    .context("Expected a finite number")?
                    .to_le_bytes(),
            ),
            "Bool" => out.extend(
                if value.as_bool().context("Expected a boolean")? {
                    0x997275b5u32
                } else {
                    0xbc799737u32
                }
                .to_le_bytes(),
            ),
            "true" => {
                if value.as_bool() != Some(true) {
                    bail!("Expected true");
                }
            }
            "string" => write_bytes(value.as_str().context("Expected a string")?.as_bytes(), out)?,
            "bytes" => write_bytes(&json_bytes(value)?, out)?,
            "int128" | "int256" => {
                let bytes = json_bytes(value)?;
                if bytes.len() != if kind == "int128" { 16 } else { 32 } {
                    bail!("Invalid fixed integer byte length");
                }
                out.extend(bytes);
            }
            _ => {
                let name = value
                    .get("_")
                    .and_then(Value::as_str)
                    .context("Telegram objects require their _ constructor")?;
                let def = self.definition(name)?;
                if !kind.starts_with('!') && kind != "Object" && kind != "X" && def.result != kind {
                    bail!("Constructor {name} does not construct {kind}");
                }
                self.encode_definition(def, value, out, depth + 1)?;
            }
        }
        Ok(())
    }
    pub fn encode_object(&self, value: &Value) -> Result<Vec<u8>> {
        let name = value
            .get("_")
            .and_then(Value::as_str)
            .context("Missing constructor")?;
        let mut out = Vec::new();
        self.encode_definition(self.definition(name)?, value, &mut out, 0)?;
        Ok(out)
    }
}
pub fn json_bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(value) = value.as_str() {
        return base64::engine::general_purpose::STANDARD
            .decode(value)
            .context("Invalid base64 bytes");
    }
    let values = value
        .as_array()
        .context("Expected byte array or base64 string")?;
    if values.len() > MAX_BYTES {
        bail!("Byte array exceeds budget");
    }
    values
        .iter()
        .map(|v| {
            u8::try_from(v.as_u64().context("Expected byte integer")?).context("Byte exceeds 255")
        })
        .collect()
}
fn write_bytes(bytes: &[u8], out: &mut Vec<u8>) -> Result<()> {
    if bytes.len() > 0xff_ffff {
        bail!("Telegram byte string exceeds protocol limit");
    }
    let header = if bytes.len() < 254 {
        out.push(bytes.len() as u8);
        1
    } else {
        out.push(254);
        out.extend(&(bytes.len() as u32).to_le_bytes()[..3]);
        4
    };
    out.extend(bytes);
    out.resize(out.len() + (4 - (header + bytes.len()) % 4) % 4, 0);
    Ok(())
}
