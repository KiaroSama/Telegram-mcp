//! Rich-message blocks are rendered as safe static HTML, never embedded scripts.

use super::render::{html_escape, relative_href, safe_href};
use anyhow::{Result, bail};
use serde_json::Value;

fn children(value: &Value, key: &str, files: &Value, depth: usize) -> Result<String> {
    let mut result = String::new();
    for item in value[key].as_array().into_iter().flatten() {
        result.push_str(&block(item, files, depth + 1)?);
    }
    Ok(result)
}

fn media_link(value: &Value, key: &str, files: &Value, image: bool) -> String {
    let id = value[key]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value[key].to_string());
    if let Some(path) = files
        .get(&id)
        .and_then(Value::as_str)
        .and_then(relative_href)
    {
        if image {
            return format!(
                "<img alt=\"Telegram media {}\" loading=\"lazy\" src=\"{}\">",
                html_escape(&id),
                html_escape(&path)
            );
        }
        return format!(
            "<a href=\"{}\">Media {}</a>",
            html_escape(&path),
            html_escape(&id)
        );
    }
    format!("<span>Media {} (not downloaded)</span>", html_escape(&id))
}

pub fn text(value: &Value, files: &Value, depth: usize) -> Result<String> {
    if depth > 64 {
        bail!("Rich-text nesting exceeds 64 levels");
    }
    if value.is_null() {
        return Ok(String::new());
    }
    if let Some(value) = value.as_str() {
        return Ok(html_escape(value));
    }
    let nested = || text(&value["text"], files, depth + 1);
    Ok(match value["_"].as_str().unwrap_or_default() {
        "textEmpty" => String::new(),
        "textPlain" => html_escape(value["text"].as_str().unwrap_or("")),
        "textConcat" => {
            let mut result = String::new();
            for part in value["texts"].as_array().into_iter().flatten() {
                result.push_str(&text(part, files, depth + 1)?);
            }
            result
        }
        "textBold" => format!("<strong>{}</strong>", nested()?),
        "textItalic" => format!("<em>{}</em>", nested()?),
        "textUnderline" => format!("<u>{}</u>", nested()?),
        "textStrike" => format!("<s>{}</s>", nested()?),
        "textFixed" => format!("<code>{}</code>", nested()?),
        "textSubscript" => format!("<sub>{}</sub>", nested()?),
        "textSuperscript" => format!("<sup>{}</sup>", nested()?),
        "textMarked" => format!("<mark>{}</mark>", nested()?),
        "textSpoiler" => format!("<details><summary>Spoiler</summary>{}</details>", nested()?),
        "textUrl" | "textEmail" | "textPhone" => {
            let target = match value["_"].as_str() {
                Some("textEmail") => format!("mailto:{}", value["email"].as_str().unwrap_or("")),
                Some("textPhone") => format!("tel:{}", value["phone"].as_str().unwrap_or("")),
                _ => value["url"].as_str().unwrap_or("").into(),
            };
            match safe_href(&target) {
                Some(href) => format!(
                    "<a rel=\"noreferrer noopener\" href=\"{}\">{}</a>",
                    html_escape(&href),
                    nested()?
                ),
                None => nested()?,
            }
        }
        "textImage" => media_link(value, "document_id", files, true),
        "textCustomEmoji" => {
            let link = media_link(value, "document_id", files, true);
            if link.starts_with("<img") {
                link
            } else {
                html_escape(value["alt"].as_str().unwrap_or(""))
            }
        }
        "textMath" => format!(
            "<code class=\"math\">{}</code>",
            html_escape(value["source"].as_str().unwrap_or(""))
        ),
        "textDiff" => format!(
            "<del>{}</del><ins>{}</ins>",
            text(&value["old_text"], files, depth + 1)?,
            nested()?
        ),
        "textDate" => format!(
            "<time data-unix=\"{}\">{}</time>",
            value["date"].as_i64().unwrap_or(0),
            nested()?
        ),
        "textAnchor" | "textMention" | "textHashtag" | "textBotCommand" | "textCashtag"
        | "textAutoUrl" | "textAutoEmail" | "textAutoPhone" | "textBankCard"
        | "textMentionName" | "textButton" | "textTonAddress" => nested()?,
        _ => {
            if value.get("text").is_some() {
                nested()?
            } else {
                html_escape(&serde_json::to_string(value)?)
            }
        }
    })
}

pub fn block(value: &Value, files: &Value, depth: usize) -> Result<String> {
    if depth > 64 {
        bail!("Rich-message nesting exceeds 64 levels");
    }
    let body = || text(&value["text"], files, depth + 1);
    Ok(match value["_"].as_str().unwrap_or_default() {
        "pageBlockTitle" | "pageBlockHeading1" => format!("<h1>{}</h1>", body()?),
        "pageBlockHeader" | "pageBlockHeading2" => format!("<h2>{}</h2>", body()?),
        "pageBlockSubheader" | "pageBlockHeading3" => format!("<h3>{}</h3>", body()?),
        "pageBlockHeading4" => format!("<h4>{}</h4>", body()?),
        "pageBlockHeading5" => format!("<h5>{}</h5>", body()?),
        "pageBlockHeading6" => format!("<h6>{}</h6>", body()?),
        "pageBlockSubtitle" | "pageBlockParagraph" | "pageBlockFooter" | "pageBlockKicker"
        | "pageBlockThinking" => format!("<p>{}</p>", body()?),
        "pageBlockPreformatted" => format!("<pre>{}</pre>", body()?),
        "pageBlockDivider" => "<hr>".into(),
        "pageBlockAuthorDate" => format!(
            "<p>{} — {}</p>",
            text(&value["author"], files, depth + 1)?,
            value["published_date"].as_i64().unwrap_or(0)
        ),
        "pageBlockBlockquote" | "pageBlockPullquote" => format!(
            "<blockquote>{}<footer>{}</footer></blockquote>",
            body()?,
            text(&value["caption"], files, depth + 1)?
        ),
        "pageBlockBlockquoteBlocks" => format!(
            "<blockquote>{}{}</blockquote>",
            children(value, "blocks", files, depth)?,
            text(&value["caption"], files, depth + 1)?
        ),
        "pageBlockCover" => block(&value["cover"], files, depth + 1)?,
        "pageBlockCollage" | "pageBlockSlideshow" => format!(
            "<section>{}{}</section>",
            children(value, "items", files, depth)?,
            caption(&value["caption"], files, depth)?
        ),
        "pageBlockList" | "pageBlockOrderedList" => {
            let ordered = value["_"].as_str() == Some("pageBlockOrderedList");
            let tag = if ordered { "ol" } else { "ul" };
            let start = value["start"].as_i64().unwrap_or(1);
            let reverse = if value["reversed"] == true {
                " reversed"
            } else {
                ""
            };
            let mut result = if ordered {
                format!("<{tag} start=\"{start}\"{reverse}>")
            } else {
                format!("<{tag}>")
            };
            for item in value["items"].as_array().into_iter().flatten() {
                result.push_str("<li>");
                if item["checkbox"] == true {
                    result.push_str(if item["checked"] == true {
                        "☑ "
                    } else {
                        "☐ "
                    });
                }
                result.push_str(&if item.get("blocks").is_some() {
                    children(item, "blocks", files, depth + 1)?
                } else {
                    text(&item["text"], files, depth + 1)?
                });
                result.push_str("</li>");
            }
            result.push_str(&format!("</{tag}>"));
            result
        }
        "pageBlockTable" => {
            let mut result = format!(
                "<table><caption>{}</caption>",
                text(&value["title"], files, depth + 1)?
            );
            for row in value["rows"].as_array().into_iter().flatten() {
                result.push_str("<tr>");
                for cell in row["cells"].as_array().into_iter().flatten() {
                    let tag = if cell["header"] == true { "th" } else { "td" };
                    let colspan = cell["colspan"].as_u64().unwrap_or(1).clamp(1, 100);
                    let rowspan = cell["rowspan"].as_u64().unwrap_or(1).clamp(1, 100);
                    result.push_str(&format!(
                        "<{tag} colspan=\"{colspan}\" rowspan=\"{rowspan}\">{}</{tag}>",
                        text(&cell["text"], files, depth + 1)?
                    ));
                }
                result.push_str("</tr>");
            }
            result.push_str("</table>");
            result
        }
        "pageBlockDetails" => format!(
            "<details{}><summary>{}</summary>{}</details>",
            if value["open"] == true { " open" } else { "" },
            text(&value["title"], files, depth + 1)?,
            children(value, "blocks", files, depth)?
        ),
        "pageBlockPhoto" => format!(
            "<figure>{}{}</figure>",
            media_link(value, "photo_id", files, true),
            caption(&value["caption"], files, depth)?
        ),
        "pageBlockVideo" | "pageBlockAudio" | "pageBlockDocument" => {
            let key = match value["_"].as_str() {
                Some("pageBlockVideo") => "video_id",
                Some("pageBlockAudio") => "audio_id",
                _ => "document_id",
            };
            format!(
                "<figure>{}{}</figure>",
                media_link(value, key, files, false),
                caption(&value["caption"], files, depth)?
            )
        }
        "pageBlockEmbed" | "pageBlockEmbedPost" => {
            let mut result = String::new();
            if let Some(url) = value["url"].as_str().and_then(safe_href) {
                result.push_str(&format!(
                    "<a href=\"{}\" rel=\"noreferrer noopener\">{}</a>",
                    html_escape(&url),
                    html_escape(&url)
                ));
            }
            if let Some(html) = value["html"].as_str() {
                result.push_str(&format!("<pre>{}</pre>", html_escape(html)));
            }
            result.push_str(&children(value, "blocks", files, depth)?);
            result
        }
        "pageBlockMath" => format!(
            "<pre class=\"math\">{}</pre>",
            html_escape(value["source"].as_str().unwrap_or(""))
        ),
        "pageBlockAnchor" => String::new(),
        _ => format!(
            "<details><summary>Telegram block {}</summary><pre>{}</pre></details>",
            html_escape(value["_"].as_str().unwrap_or("unknown")),
            html_escape(&serde_json::to_string_pretty(value)?)
        ),
    })
}

fn caption(value: &Value, files: &Value, depth: usize) -> Result<String> {
    Ok(format!(
        "<figcaption>{}{}</figcaption>",
        text(&value["text"], files, depth + 1)?,
        text(&value["credit"], files, depth + 1)?
    ))
}

pub fn message(value: &Value, files: &Value) -> Result<String> {
    children(value, "blocks", files, 0)
}
