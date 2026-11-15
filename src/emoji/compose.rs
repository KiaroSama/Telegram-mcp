use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub fn u16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub fn byte_index(text: &str, units: usize) -> Result<usize> {
    crate::export::render::utf16_byte_offset(text, units)
}

fn kind(entity: &Value) -> &str {
    entity["type"]
        .as_str()
        .unwrap_or_else(|| match entity["_"].as_str().unwrap_or("") {
            "messageEntityCustomEmoji" => "custom_emoji",
            "messageEntityBlockquote" => "blockquote",
            "messageEntityPre" => "pre",
            "messageEntityCode" => "code",
            other => other,
        })
}

pub fn insert_emoji(
    text: &str,
    entities: &[Value],
    line: usize,
    glyph: &str,
    document_id: &str,
    where_: &str,
) -> Result<(String, Vec<Value>)> {
    let lines: Vec<&str> = text.split('\n').collect();
    ensure!(
        line > 0 && line <= lines.len(),
        "The message has {} line(s); line {line} does not exist. Lines are 1-based",
        lines.len()
    );
    ensure!(
        ["start", "end"].contains(&where_),
        "where must be start or end"
    );
    ensure!(!glyph.is_empty(), "The fallback glyph must not be empty");
    ensure!(
        document_id.parse::<i64>().is_ok_and(|value| value > 0),
        "document_id must be a positive 64-bit decimal string"
    );
    let prefix = lines[..line - 1]
        .iter()
        .map(|line| u16_len(line) + 1)
        .sum::<usize>();
    let at = prefix
        + if where_ == "start" {
            0
        } else {
            u16_len(lines[line - 1])
        };
    let width = u16_len(glyph);
    let mut moved = Vec::with_capacity(entities.len() + 1);
    for entity in entities {
        let mut item = entity.clone();
        ensure!(item.is_object(), "An entity must be an object");
        if let (Some(offset), Some(length)) = (
            entity.get("offset").filter(|v| !v.is_null()),
            entity.get("length").filter(|v| !v.is_null()),
        ) {
            let start = offset
                .as_u64()
                .context("Entity offset must be a nonnegative integer")?
                as usize;
            let length = length
                .as_u64()
                .context("Entity length must be a nonnegative integer")?
                as usize;
            let end = start
                .checked_add(length)
                .context("Entity range overflows")?;
            byte_index(text, start)?;
            byte_index(text, end)?;
            if start >= at {
                item["offset"] = start
                    .checked_add(width)
                    .context("Entity offset overflows")?
                    .into();
            } else if end > at
                || (end == at && ["blockquote", "pre", "code"].contains(&kind(entity)))
            {
                item["length"] = length
                    .checked_add(width)
                    .context("Entity length overflows")?
                    .into();
            }
        }
        moved.push(item);
    }
    moved.push(
        json!({"type":"custom_emoji","offset":at,"length":width,"custom_emoji_id":document_id}),
    );
    moved.sort_by_key(|entity| entity["offset"].as_u64().unwrap_or(0));
    let index = byte_index(text, at)?;
    Ok((
        format!("{}{}{}", &text[..index], glyph, &text[index..]),
        moved,
    ))
}

pub fn best_frame(frames: &[Value]) -> usize {
    frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| {
            !frame["blank"]
                .as_bool()
                .unwrap_or_else(|| frame["transparent"].as_bool().unwrap_or(false))
        })
        .max_by(|(li, left), (ri, right)| {
            left["bytes"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&right["bytes"].as_u64().unwrap_or(0))
                .then_with(|| ri.cmp(li))
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub fn line_of_offset(text: &str, offset: usize) -> Result<usize> {
    Ok(text[..byte_index(text, offset)?]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1)
}

pub fn describe_lines(text: &str, entities: &[Value]) -> Result<Vec<Value>> {
    let mut on_line = std::collections::BTreeMap::<usize, Vec<String>>::new();
    for entity in entities {
        if kind(entity) == "custom_emoji"
            && let Some(offset) = entity["offset"].as_u64()
        {
            let id = entity
                .get("custom_emoji_id")
                .or_else(|| entity.get("document_id"))
                .unwrap_or(&Value::Null);
            on_line
                .entry(line_of_offset(text, offset as usize)?)
                .or_default()
                .push(
                    id.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| id.to_string()),
                );
        }
    }
    Ok(text.split('\n').enumerate().map(|(index,line)|json!({"line":index+1,"text":line,"emoji":on_line.get(&(index+1)).cloned().unwrap_or_default()})).collect())
}
