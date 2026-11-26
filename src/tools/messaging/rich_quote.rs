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
                    if let Some(credit) = block["caption"].get("credit") {
                        add(out, credit, index, None, None);
                    }
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
        for (byte, _) in source
            .text
            .char_indices()
            .filter(|(byte, _)| source.text[*byte..].starts_with(wanted))
        {
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

pub(super) fn view(response: &Value, peer: &Value, mid: i64) -> Result<Value> {
    let m = response["messages"]
        .as_array()
        .and_then(|ms| {
            ms.iter()
                .find(|m| m["id"] == mid && super::helpers::same_peer(&m["peer_id"], peer))
        })
        .context("Rich message was not found in the requested chat")?;
    let rich = &m["rich_message"];
    let blocks = rich["blocks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut links = std::collections::BTreeSet::new();
    let mut media = vec![];
    fn walk(
        node: &Value,
        rich: &Value,
        links: &mut std::collections::BTreeSet<String>,
        media: &mut Vec<Value>,
    ) {
        match node {
            Value::Object(obj) => {
                let link = match node["_"].as_str() {
                    Some("textUrl") => node["url"].as_str().map(str::to_owned),
                    Some("textEmail") => node["email"].as_str().map(|s| format!("mailto:{s}")),
                    Some("textPhone") => node["phone"].as_str().map(|s| format!("tel:{s}")),
                    _ => None,
                };
                if let Some(link) = link.filter(|s| {
                    !s.chars().any(char::is_control)
                        && url::Url::parse(s).is_ok_and(|u| {
                            matches!(u.scheme(), "http" | "https" | "mailto" | "tel")
                        })
                }) {
                    links.insert(link);
                }
                for (key, kind, table) in [
                    ("photo_id", "photo", "photos"),
                    ("document_id", "document", "documents"),
                    ("video_id", "document", "documents"),
                    ("audio_id", "document", "documents"),
                ] {
                    if let Some(id) = obj.get(key).filter(|v| !v.is_null()) {
                        let available = rich[table]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|d| d["id"] == *id);
                        media.push(json!({"kind":kind,"id":id,"available":available}));
                    }
                }
                for v in obj.values() {
                    walk(v, rich, links, media)
                }
            }
            Value::Array(xs) => {
                for v in xs {
                    walk(v, rich, links, media)
                }
            }
            _ => (),
        }
    }
    for block in blocks {
        walk(block, rich, &mut links, &mut media);
    }
    let quotes=sources(blocks).into_iter().map(|s|json!({"block_index":s.block,"row":s.row,"column":s.column,"offset":s.base,"text":s.text,"length":s.text.encode_utf16().count()})).collect::<Vec<_>>();
    let records=blocks.iter().map(|b|{let mut row=b.clone();let kind=b["_"].as_str().unwrap_or("").trim_start_matches("pageBlock").to_lowercase();row["type"]=json!(kind);if kind=="table"{let cells=b["rows"].as_array().into_iter().flatten().map(|r|r["cells"].as_array().into_iter().flatten().map(|c|json!({"text":plain(&c["text"]).unwrap_or_default(),"header":c["header"]==true,"colspan":c["colspan"],"rowspan":c["rowspan"]})).collect::<Vec<_>>()).collect::<Vec<_>>();let md=cells.iter().map(|r|format!("| {} |",r.iter().map(|c|c["text"].as_str().unwrap_or_default().replace('|',"\\|")).collect::<Vec<_>>().join(" | "))).collect::<Vec<_>>().join("\n");row["row_count"]=json!(cells.len());row["column_count"]=json!(cells.iter().map(Vec::len).max().unwrap_or(0));row["rows"]=json!(cells);row["markdown"]=json!(md);}else if b["text"].is_object(){row["rich_text"]=b["text"].clone();row["text"]=json!(plain(&b["text"]).unwrap_or_default());}row}).collect::<Vec<_>>();
    let reply = &m["reply_to"];
    let fragments = reply["quote_rich"].as_array();
    let quote = if let Some(parts) = fragments.filter(|xs| !xs.is_empty()) {
        Some(
            json!({"rich":true,"text":parts.iter().map(plain).collect::<Result<Vec<_>>>()?.join("\n"),"fragments":parts,"offset":reply["quote_offset"],"message_id":reply["reply_to_msg_id"]}),
        )
    } else {
        reply.get("quote_text").filter(|v|!v.is_null()).map(|v|json!({"text":v,"offset":reply["quote_offset"],"entities":reply["quote_entities"],"message_id":reply["reply_to_msg_id"]}))
    };
    let mut view = json!({"message_id":mid,"content_type":rich.get("_"),"block_count":blocks.len(),"blocks":records,"links":links,"media":media,"quote_sources":quotes,"reply_quote":quote});
    if let Some(link) = m.get("permalink") {
        view["permalink"] = link.clone();
    }
    Ok(view)
}
