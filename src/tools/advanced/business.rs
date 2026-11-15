use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "get_business_settings"
            | "set_business_hours"
            | "set_business_away_message"
            | "set_business_greeting"
            | "set_business_intro"
            | "set_business_location"
            | "list_business_chat_links"
            | "create_business_chat_link"
            | "edit_business_chat_link"
            | "delete_business_chat_link"
            | "resolve_business_chat_link"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
/// Normalize hours across the week boundary, preserving Sunday-to-Monday hours.
/// Telegram accepts at most 28 merged intervals, each at most one week long.
pub(super) fn normalize(raw: &Value) -> Result<Vec<(i64, i64)>> {
    let input = raw.as_array().ok_or_else(|| {
        anyhow::anyhow!("intervals must be a list of [start_minute, end_minute] pairs")
    })?;
    ensure!(input.len() <= 1000, "Too many work-hour intervals");
    let mut pairs = Vec::new();
    for pair in input {
        let v = pair
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Each interval must be [start_minute, end_minute]"))?;
        ensure!(v.len() == 2, "Each interval needs two values");
        let start = v[0]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Work-hour minutes must be whole integers"))?;
        let end = v[1]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Work-hour minutes must be whole integers"))?;
        ensure!(
            (0..=10080).contains(&start) && (1..=11520).contains(&end) && end > start,
            "Work hours are minutes of the week: start 0..10080, end 1..11520, and end after start"
        );
        pairs.push((start, end));
    }
    loop {
        pairs.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (start, end) in &pairs {
            let end = (*end).min(*start + 10080);
            if let Some(last) = merged.last_mut()
                && *start <= last.1
            {
                last.1 = last.1.max(end);
                continue;
            }
            merged.push((*start, end));
        }
        if merged.len() > 1 {
            let first = merged[0];
            let last_index = merged.len() - 1;
            let last = merged[last_index];
            if last.1 > 10080 && last.1 - 10080 >= first.0 {
                if first.1 + 10080 < 11520 {
                    merged[last_index].1 = last.1.max(first.1 + 10080);
                    merged.remove(0);
                } else {
                    merged[0] = (0, first.1.max(last.1 - 10080));
                    merged[last_index].1 = 10080;
                }
            }
        }
        if merged == pairs {
            ensure!(
                merged.len() <= 28,
                "Telegram allows at most 28 merged work-hour intervals"
            );
            return Ok(merged);
        }
        pairs = merged;
    }
}
fn recipients(args: &Value) -> Result<Value> {
    let names = args
        .get("recipients")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_array()
                .ok_or_else(|| anyhow::anyhow!("recipients must be a list"))
        })
        .transpose()?;
    let mut out = json!({"_":"inputBusinessRecipients"});
    let chosen: Vec<_> = match names {
        Some(v) if !v.is_empty() => v
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("Recipient groups must be strings"))
            })
            .collect::<Result<_>>()?,
        _ => vec!["existing_chats", "new_chats", "contacts", "non_contacts"],
    };
    for n in chosen {
        ensure!(
            matches!(
                n,
                "existing_chats" | "new_chats" | "contacts" | "non_contacts"
            ),
            "Unknown recipient group"
        );
        out[n] = json!(true);
    }
    Ok(out)
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    match name {
        "get_business_settings" => {
            let full = api
                .invoke("users.getFullUser", json!({"id":{"_":"inputUserSelf"}}))
                .await?;
            let u = &full["full_user"];
            let mut out = json!({});
            for field in [
                "business_work_hours",
                "business_location",
                "business_greeting_message",
                "business_away_message",
                "business_intro",
            ] {
                out[field] = u[field].clone();
            }
            Ok(out)
        }
        "set_business_hours" => {
            let Some(raw) = args.get("intervals").filter(|v| !v.is_null()) else {
                return api
                    .invoke("account.updateBusinessWorkHours", json!({}))
                    .await;
            };
            let timezone = a::text(args, "timezone_id")?;
            nonempty(timezone, 128, "Timezone ID")?;
            let hours = normalize(raw)?;
            if hours.is_empty() {
                return api
                    .invoke("account.updateBusinessWorkHours", json!({}))
                    .await;
            }
            let weekly: Vec<_> = hours
                .iter()
                .map(|(s, e)| json!({"_":"businessWeeklyOpen","start_minute":s,"end_minute":e}))
                .collect();
            api.invoke("account.updateBusinessWorkHours",json!({"business_work_hours":{"_":"businessWorkHours","timezone_id":timezone,"weekly_open":weekly}})).await
        }
        "set_business_away_message" | "set_business_greeting" => {
            let method = if name == "set_business_greeting" {
                "account.updateBusinessGreetingMessage"
            } else {
                "account.updateBusinessAwayMessage"
            };
            let Some(shortcut) = args.get("shortcut_id").filter(|v| !v.is_null()) else {
                return api.invoke(method, json!({})).await;
            };
            let sid = positive(shortcut)?;
            let recipient = recipients(args)?;
            let message = if name == "set_business_greeting" {
                let days = a::integer(args, "no_activity_days", 7)?;
                ensure!(
                    [7, 14, 21, 28].contains(&days),
                    "no_activity_days must be 7, 14, 21 or 28"
                );
                json!({"_":"inputBusinessGreetingMessage","shortcut_id":sid,"recipients":recipient,"no_activity_days":days})
            } else {
                let schedule = match string(args, "schedule", "always")? {
                    "always" => json!({"_":"businessAwayMessageScheduleAlways"}),
                    "outside_hours" => json!({"_":"businessAwayMessageScheduleOutsideWorkHours"}),
                    "custom" => {
                        let start = timestamp(args, "start_date")?;
                        let end = timestamp(args, "end_date")?;
                        ensure!(end > start, "end_date must be after start_date");
                        json!({"_":"businessAwayMessageScheduleCustom","start_date":start,"end_date":end})
                    }
                    _ => bail!("schedule must be always, outside_hours or custom"),
                };
                json!({"_":"inputBusinessAwayMessage","shortcut_id":sid,"schedule":schedule,"recipients":recipient,"offline_only":a::boolean(args,"offline_only",false)?})
            };
            api.invoke(method, json!({"message":message})).await
        }
        "set_business_intro" => {
            let Some(title) = args.get("title").filter(|v| !v.is_null()) else {
                return api.invoke("account.updateBusinessIntro", json!({})).await;
            };
            let title = title
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("title must be a string"))?;
            ensure!(
                title.encode_utf16().count() <= 32,
                "Business intro title exceeds 32 UTF-16 units"
            );
            let description = string(args, "description", "")?;
            ensure!(
                description.encode_utf16().count() <= 70,
                "Business intro description exceeds 70 UTF-16 units"
            );
            let mut intro =
                json!({"_":"inputBusinessIntro","title":title,"description":description});
            let did = args.get("sticker_document_id").filter(|v| !v.is_null());
            let hash = args.get("sticker_access_hash").filter(|v| !v.is_null());
            ensure!(
                did.is_some() == hash.is_some(),
                "Give both sticker_document_id and sticker_access_hash"
            );
            if let (Some(d), Some(h)) = (did, hash) {
                intro["sticker"] = json!({"_":"inputDocument","id":positive(d)?.to_string(),"access_hash":id(h)?.to_string(),"file_reference":[]});
            }
            api.invoke("account.updateBusinessIntro", json!({"intro":intro}))
                .await
        }
        "set_business_location" => {
            let Some(address) = args.get("address").filter(|v| !v.is_null()) else {
                return api
                    .invoke("account.updateBusinessLocation", json!({}))
                    .await;
            };
            let address = address
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("address must be a string"))?;
            nonempty(address, 96, "Business address")?;
            let lat = args.get("latitude").filter(|v| !v.is_null());
            let lon = args.get("longitude").filter(|v| !v.is_null());
            ensure!(
                lat.is_some() == lon.is_some(),
                "Give both latitude and longitude"
            );
            let mut p = json!({"address":address});
            if let (Some(lat), Some(lon)) = (lat, lon) {
                let lat = lat
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("latitude must be numeric"))?;
                let lon = lon
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("longitude must be numeric"))?;
                ensure!(
                    (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon),
                    "Coordinates are outside valid latitude/longitude ranges"
                );
                p["geo_point"] = json!({"_":"inputGeoPoint","lat":lat,"long":lon});
            }
            api.invoke("account.updateBusinessLocation", p).await
        }
        "list_business_chat_links" => api.invoke("account.getBusinessChatLinks", json!({})).await,
        "create_business_chat_link" | "edit_business_chat_link" => {
            let text = a::text(args, "text")?;
            nonempty(text, 4096, "Business link text")?;
            let mut link = json!({"_":"inputBusinessChatLink","message":text});
            copy_optional(args, &mut link, &["title"]);
            let mut p = json!({"link":link});
            let method = if name == "edit_business_chat_link" {
                p["slug"] = json!(slug(a::text(args, "slug")?, "message")?);
                "account.editBusinessChatLink"
            } else {
                "account.createBusinessChatLink"
            };
            api.invoke(method, p).await
        }
        "delete_business_chat_link" | "resolve_business_chat_link" => {
            let method = if name == "delete_business_chat_link" {
                "account.deleteBusinessChatLink"
            } else {
                "account.resolveBusinessChatLink"
            };
            api.invoke(
                method,
                json!({"slug":slug(a::text(args,"slug")?,"message")?}),
            )
            .await
        }
        _ => bail!("Unknown business operation"),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_business.rs"]
mod tests;
