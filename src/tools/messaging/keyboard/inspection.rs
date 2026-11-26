use super::*;
pub(super) fn visible(m: &Value) -> bool {
    m["out"] != true && (m["reply_markup"]["selective"] != true || m["mentioned"] == true)
}
fn describe(api: &dyn Telegram, a: &Value, p: &Value, m: &Value) -> Result<Vec<Value>> {
    let mut out = vec![];
    let all = buttons(m);
    let glass = m["reply_markup"]["_"] == "replyInlineMarkup";
    let mut counts = std::collections::BTreeMap::new();
    for b in &all {
        let raw = b["text"].as_str().unwrap_or_default();
        let (count, first, collision) = counts.entry(label(raw)).or_insert((0usize, raw, false));
        *count += 1;
        *collision |= *first != raw;
    }
    let mut index = 0;
    for (r, row) in m["reply_markup"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (c, b) in row["buttons"].as_array().into_iter().flatten().enumerate() {
            let raw = b["text"].as_str().unwrap_or_default();
            let text = label(raw);
            let (duplicate, collision) = counts
                .get(&text)
                .map(|(n, _, c)| (*n > 1, *c))
                .unwrap_or_default();
            let k = kind(b);
            let route = if collision {
                None
            } else {
                match k {
                    "inlineButtonTypeCallback" if b["type"]["requires_password"] != true => {
                        Some("click_button")
                    }
                    "buttonTypeDefault" => Some("press_reply_button"),
                    "buttonTypeRequestPeer"
                    | "buttonTypeRequestPoll"
                    | "buttonTypeRequestPhone"
                    | "buttonTypeRequestGeoLocation" => Some("answer_reply_button"),
                    _ => None,
                }
            };
            let mut item = json!({"index":index,"button_index":index,"row":r,"column":c,"text":text,"kind":k,"type":b["type"],"style":b["style"],"keyboard_type":m["reply_markup"]["_"],"is_glass":glass,"pressable":route.is_some(),"press_with":route,"duplicate_label":duplicate,"text_collision":collision,"text_altered":raw!=text});
            if route == Some("click_button") {
                item["press_token"] = json!(token(snapshot(api, a, p, id(m, "id")?, index, b))?);
            }
            copy_optional(
                &mut item,
                &b["type"],
                &[
                    ("url", "url"),
                    ("copy_text", "copy_text"),
                    ("query", "query"),
                ],
            );
            if let Some(icon) = b["style"]
                .get("icon")
                .or_else(|| b["style"].get("icon_document_id"))
                .filter(|v| !v.is_null())
            {
                item["icon_custom_emoji_id"] = icon.clone();
            }
            out.push(item);
            index += 1;
        }
    }
    Ok(out)
}
pub(super) async fn inspect(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let p = args::peer(api, a, "chat_id").await?;
    let (mut m, recent) = keyboard_message(api, &p, a).await?;
    if name == "list_inline_buttons" && a["message_id"].is_null() {
        if let Some(found) = recent
            .iter()
            .find(|m| m["reply_markup"]["_"] == "replyInlineMarkup")
        {
            m = found.clone();
        } else {
            return Ok(json!({"message_id":null,"buttons":[]}));
        }
    }
    let mut primary = describe(api, a, &p, &m)?;
    let active = recent.iter().find(|m| {
        visible(m)
            && matches!(
                m["reply_markup"]["_"].as_str(),
                Some("replyKeyboardMarkup" | "replyKeyboardHide" | "replyKeyboardForceReply")
            )
    });
    let mut secondary = active
        .map(|m| describe(api, a, &p, m))
        .transpose()?
        .unwrap_or_default();
    if args::boolean(a, "resolve_icons", true)? {
        let ids = primary
            .iter()
            .chain(&secondary)
            .filter_map(|v| v.get("icon_custom_emoji_id"))
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .collect::<std::collections::BTreeSet<_>>();
        if !ids.is_empty() {
            let docs = api
                .invoke(
                    "messages.getCustomEmojiDocuments",
                    json!({"document_id":ids}),
                )
                .await
                .unwrap_or_default();
            for item in primary.iter_mut().chain(&mut secondary) {
                if let Some(id) = item.get("icon_custom_emoji_id") {
                    let doc = docs
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|d| d["id"] == *id);
                    let alt = doc.and_then(|d| {
                        d["attributes"]
                            .as_array()?
                            .iter()
                            .find(|a| a["_"] == "documentAttributeCustomEmoji")?
                            .get("alt")
                    });
                    item["icon"] = json!({"resolved":doc.is_some(),"alt":alt,"mime_type":doc.map(|d|&d["mime_type"])});
                }
            }
        }
    }
    Ok(
        json!({"message_id":m["id"],"buttons":primary,"keyboard_type":m["reply_markup"]["_"],"is_glass":m["reply_markup"]["_"]=="replyInlineMarkup","active_reply_keyboard":active.map(|m|json!({"message_id":m["id"],"markup":m["reply_markup"],"buttons":secondary,"state":match m["reply_markup"]["_"].as_str(){Some("replyKeyboardMarkup")=>"shown",Some("replyKeyboardHide")=>"hidden",_=>"force_reply"}}))}),
    )
}
