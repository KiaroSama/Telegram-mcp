use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub fn plain(node: &Value) -> Result<String> {
    match node["_"].as_str().unwrap_or("") {
        "textEmpty" => Ok(String::new()),
        "textPlain" => Ok(node["text"].as_str().unwrap_or("").to_owned()),
        "textConcat" => node["texts"]
            .as_array()
            .context("Rich text concat is missing its children")?
            .iter()
            .map(plain)
            .collect::<Result<Vec<_>>>()
            .map(|parts| parts.join("")),
        "textCustomEmoji" => Ok(node["alt"].as_str().unwrap_or("").to_owned()),
        "textMath" => Ok(node["source"].as_str().unwrap_or("").to_owned()),
        "textImage" => Ok("\u{fffc}".to_owned()),
        _ if node["text"].is_object() => plain(&node["text"]),
        _ => bail!("This rich text cannot be quoted faithfully"),
    }
}
fn utf16_slice(text: &str, start: usize, end: usize) -> Result<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(
        units
            .get(start..end)
            .context("Quote offsets are outside the source")?,
    )
    .context("Quote offsets split a Unicode character")
}
fn slice(node: &Value, start: usize, end: usize) -> Result<Value> {
    let text = plain(node)?;
    let len = text.encode_utf16().count();
    ensure!(
        start <= end && end <= len,
        "Quote offsets are outside the source"
    );
    if start == 0 && end == len {
        return Ok(node.clone());
    }
    if node["_"].as_str() == Some("textPlain") {
        return Ok(json!({"_":"textPlain","text":utf16_slice(&text,start,end)?}));
    }
    if node["_"].as_str() == Some("textConcat") {
        let mut position = 0;
        let mut selected = vec![];
        for child in node["texts"]
            .as_array()
            .context("Rich concat has no children")?
        {
            let length = plain(child)?.encode_utf16().count();
            let left = start.max(position);
            let right = end.min(position + length);
            if left < right {
                selected.push(slice(child, left - position, right - position)?);
            }
            position += length;
        }
        return Ok(json!({"_":"textConcat","texts":selected}));
    }
    if node["text"].is_object() {
        let mut copy = node.clone();
        copy["text"] = slice(&node["text"], start, end)?;
        return Ok(copy);
    }
    bail!("This rich object cannot be selected partially; quote it whole")
}
struct Source {
    node: Value,
    text: String,
    block: usize,
    row: Option<usize>,
    column: Option<usize>,
    base: usize,
}
fn sources(blocks: &[Value]) -> Vec<Source> {
    fn add(
        out: &mut Vec<Source>,
        node: &Value,
        block: usize,
        row: Option<usize>,
        column: Option<usize>,
    ) {
        if let Ok(text) = plain(node)
            && !text.is_empty()
        {
            let base = out
                .last()
                .map(|s| s.base + s.text.encode_utf16().count() + 1)
                .unwrap_or(0);
            out.push(Source {
                node: node.clone(),
                text,
                block,
                row,
                column,
                base,
            });
        }
    }
    fn walk(out: &mut Vec<Source>, blocks: &[Value], top: Option<usize>) {
        for (index, block) in blocks.iter().enumerate() {
            let index = top.unwrap_or(index);
            if block["_"].as_str() == Some("pageBlockTable") {
                add(out, &block["title"], index, None, None);
                if let Some(rows) = block["rows"].as_array() {
                    for (r, row) in rows.iter().enumerate() {
                        if let Some(cells) = row["cells"].as_array() {
                            for (c, cell) in cells.iter().enumerate() {
                                add(out, &cell["text"], index, Some(r), Some(c));
                            }
                        }
                    }
                }
            } else {
                if block["text"].is_object() {
                    add(out, &block["text"], index, None, None);
                }
                for key in ["blocks", "items"] {
                    if let Some(children) = block[key].as_array() {
                        walk(out, children, Some(index));
                    }
                }
                if block["caption"].is_object() {
                    let caption = block["caption"].get("text").unwrap_or(&block["caption"]);
                    add(out, caption, index, None, None);
                }
            }
        }
    }
    let mut out = vec![];
    walk(&mut out, blocks, None);
    out
}
pub fn select(rich: &Value, a: &Value) -> Result<(Vec<Value>, usize)> {
    let wanted = a["quote_text"]
        .as_str()
        .context("quote_text is required for rich quote coordinates")?;
    ensure!(
        !wanted.is_empty() && wanted.len() <= 4096,
        "quote_text must contain 1 to 4096 UTF-8 bytes"
    );
    let coordinate = |key: &str| -> Result<Option<usize>> {
        a.get(key)
            .filter(|v| !v.is_null())
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|v| usize::try_from(v).ok())
                    .context("Quote coordinates must be nonnegative integers")
            })
            .transpose()
    };
    let offset = coordinate("quote_offset")?;
    let block = coordinate("quote_block_index")?;
    let row = coordinate("quote_row")?;
    let column = coordinate("quote_column")?;
    ensure!(
        row.is_some() == column.is_some(),
        "quote_row and quote_column must be provided together"
    );
    let sources = sources(
        rich["blocks"]
            .as_array()
            .context("This message has no rich blocks")?,
    );
    let mut matches = vec![];
    for source in sources {
        if block.is_some_and(|b| b != source.block)
            || row.is_some() && (row != source.row || column != source.column)
        {
            continue;
        }
        for (byte, _) in source.text.match_indices(wanted) {
            let start = source.text[..byte].encode_utf16().count();
            let absolute = source.base + start;
            if offset.is_some_and(|o| o != absolute) {
                continue;
            }
            matches.push((
                slice(&source.node, start, start + wanted.encode_utf16().count())?,
                absolute,
            ));
        }
    }
    ensure!(
        matches.len() == 1,
        if matches.is_empty() {
            "quote_text was not found at the specified rich location"
        } else {
            "Quote selection is ambiguous; specify block, row, column or offset"
        }
    );
    let (fragment, offset) = matches.remove(0);
    Ok((vec![fragment], offset))
}
