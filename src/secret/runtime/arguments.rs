use super::*;
use grammers_tl_types::{Deserializable, Serializable};

pub(super) fn format_message(
    text: &str,
    mode: Option<&str>,
) -> Result<(String, Option<Vec<tl::enums::MessageEntity>>)> {
    let (text, entities) = match mode.unwrap_or("plain").to_ascii_lowercase().as_str() {
        "plain" | "none" | "" => return Ok((text.into(), Some(Vec::new()))),
        "markdown" | "md" => grammers_client::parsers::parse_markdown_message(text),
        "html" => grammers_client::parsers::parse_html_message(text),
        _ => bail!("Unsupported secret-chat parse mode"),
    };
    for entity in &entities {
        ensure!(
            kiaro_secret_chat::enums::MessageEntity::from_bytes(&entity.to_bytes()).is_ok(),
            "The secret-chat layer does not support one of the message entities"
        );
    }
    Ok((text, Some(entities)))
}

pub(super) fn integer(args: &Value, key: &str) -> Option<i64> {
    args[key]
        .as_i64()
        .or_else(|| args[key].as_str()?.parse().ok())
}
pub(super) fn required_int(args: &Value, key: &str) -> Result<i64> {
    integer(args, key).with_context(|| format!("A valid {key} is required"))
}
pub(super) fn chat_id(args: &Value, key: &str) -> Result<i32> {
    crate::secret::wire_chat_id(required_int(args, key)?).context("Invalid secret-chat ID")
}
pub(super) fn lifted(id: i32) -> i64 {
    i64::from(id) - crate::secret::CHAT_OFFSET
}
pub(super) fn alias_chat(args: &Value, field: &str, alias: &str) -> Result<i32> {
    if args[field].is_null() {
        return chat_id(args, alias);
    }
    let id = chat_id(args, field)?;
    ensure!(
        args[alias].is_null() || chat_id(args, alias)? == id,
        "Secret-chat identifiers disagree"
    );
    Ok(id)
}
pub(super) fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .with_context(|| format!("{key} must be text"))
}
pub(super) fn path<'a>(args: &'a Value, key: &str) -> Option<&'a Path> {
    args[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(Path::new)
}
pub(super) fn ids(value: &Value) -> Result<Vec<i64>> {
    let items = value.as_array().context("Message IDs must be an array")?;
    ensure!(items.len() <= 1000, "Too many secret message IDs");
    items
        .iter()
        .map(|v| {
            v.as_i64()
                .or_else(|| v.as_str()?.parse().ok())
                .context("Invalid secret message ID")
        })
        .collect()
}
