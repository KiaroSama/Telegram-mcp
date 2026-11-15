use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "get_channel_statistics"
            | "analyze_chat_statistics"
            | "get_fact_check"
            | "set_fact_check"
            | "delete_fact_check"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
fn percent(current: f64, previous: f64) -> Value {
    if previous == 0.0 {
        Value::Null
    } else {
        json!(((current - previous) * 1000.0 / previous).round() / 10.0)
    }
}
fn moment(v: &Value) -> Value {
    let millis = v.as_i64().or_else(|| v.as_f64().map(|n| n as i64));
    millis
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| json!(d.to_rfc3339()))
        .unwrap_or_else(|| v.clone())
}
async fn graph(api: &dyn Telegram, value: &Value, resolve: bool, include: bool) -> Result<Value> {
    let mut current = value.clone();
    if current["_"] == "statsGraphAsync" {
        if !resolve {
            return Ok(
                json!({"status":"not_loaded","note":"Telegram returned a graph token and resolve_graphs is off"}),
            );
        }
        let token = current["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Stats graph token is missing"))?;
        match api
            .invoke("stats.loadAsyncGraph", json!({"token":token}))
            .await
        {
            Ok(v) => current = v,
            Err(_) => {
                return Ok(
                    json!({"status":"not_loaded","note":"The follow-up graph request failed"}),
                );
            }
        }
    }
    if current["_"] == "statsGraphError" {
        return Ok(json!({"status":"error","error":current["error"]}));
    }
    if current["_"] != "statsGraph" {
        return Ok(json!({"status":"not_loaded","note":"Telegram returned no graph data"}));
    }
    let raw = current["json"]["data"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Graph JSON is missing"))?;
    ensure!(
        raw.len() <= 8 * 1024 * 1024,
        "Stats graph exceeds the 8 MiB decoding budget"
    );
    let data: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Ok(json!({"status":"error","error":"Graph JSON did not parse"})),
    };
    let columns = data["columns"].as_array().cloned().unwrap_or_default();
    ensure!(
        columns.iter().all(|column| column
            .as_array()
            .is_some_and(|values| !values.is_empty() && values[0].is_string())),
        "Stats graph contains malformed columns"
    );
    ensure!(
        columns.len() <= 200
            && columns
                .iter()
                .all(|v| v.as_array().is_some_and(|v| v.len() <= 100001)),
        "Stats graph exceeds the point budget"
    );
    let series: Vec<_> = columns
        .iter()
        .filter(|v| v[0] != "x")
        .map(|c| {
            data["names"]
                .get(c[0].as_str().unwrap_or(""))
                .cloned()
                .unwrap_or_else(|| c[0].clone())
        })
        .collect();
    let axis = columns
        .iter()
        .find(|v| v[0] == "x")
        .and_then(Value::as_array);
    let mut out = json!({"status":"loaded","series":series,"points":columns.iter().filter_map(|v|v.as_array().map(|v|v.len().saturating_sub(1))).max().unwrap_or(0)});
    if let Some(x) = axis
        && x.len() > 1
    {
        out["from"] = moment(&x[1]);
        out["to"] = moment(x.last().unwrap());
    }
    if include {
        out["columns"] = json!(columns);
    }
    Ok(out)
}
pub(super) async fn resolve_graphs(
    api: &dyn Telegram,
    value: &mut Value,
    resolve: bool,
    include: bool,
) -> Result<()> {
    let fields: Vec<String> = value
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    for field in fields {
        let v = value[&field].clone();
        if matches!(
            v["_"].as_str(),
            Some("statsGraph" | "statsGraphAsync" | "statsGraphError")
        ) {
            value[&field] = graph(api, &v, resolve, include).await?;
        } else if v["_"] == "statsAbsValueAndPrev" {
            let current = v["current"].as_f64().unwrap_or(0.0);
            let previous = v["previous"].as_f64().unwrap_or(0.0);
            value[&field] = json!({"current":current,"previous":previous,"delta":current-previous});
        } else if v["_"] == "statsPercentValue" {
            let part = v["part"].as_f64().unwrap_or(0.0);
            let total = v["total"].as_f64().unwrap_or(0.0);
            value[&field] = json!({"part":part,"total":total,"percent":if total==0.0{Value::Null}else{json!((part*10000.0/total).round()/100.0)}});
        }
    }
    let users = value["users"].as_array().cloned().unwrap_or_default();
    for field in ["top_posters", "top_admins", "top_inviters"] {
        if let Some(rows) = value[field].as_array_mut() {
            for row in rows {
                if let Some(user) = users.iter().find(|u| u["id"] == row["user_id"]) {
                    let deleted = user["deleted"].as_bool() == Some(true);
                    let name = if deleted {
                        "Deleted Account".into()
                    } else {
                        [
                            user["first_name"].as_str().unwrap_or(""),
                            user["last_name"].as_str().unwrap_or(""),
                        ]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                    };
                    row["name"] = json!(name);
                    if deleted {
                        row["deleted_account"] = json!(true);
                    }
                }
            }
        }
    }
    Ok(())
}
fn columns(graph: &Value) -> (Vec<Value>, Vec<(String, Vec<f64>)>) {
    let cols = graph["columns"].as_array().cloned().unwrap_or_default();
    let names = graph["series"].as_array().cloned().unwrap_or_default();
    let x = cols
        .iter()
        .find(|c| c[0] == "x")
        .and_then(Value::as_array)
        .map(|v| v[1..].to_vec())
        .unwrap_or_default();
    let ys = cols
        .iter()
        .filter(|c| c[0] != "x")
        .enumerate()
        .filter_map(|(i, c)| {
            c.as_array().map(|v| {
                (
                    names
                        .get(i)
                        .and_then(Value::as_str)
                        .unwrap_or(c[0].as_str().unwrap_or("series"))
                        .into(),
                    v[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)).collect(),
                )
            })
        })
        .collect();
    (x, ys)
}
fn day(v: &Value) -> Value {
    let v = moment(v);
    json!(
        v.as_str()
            .unwrap_or("")
            .chars()
            .take(10)
            .collect::<String>()
    )
}
fn daily(name: &str, graph: &Value, days: usize) -> Value {
    let (x, ys) = columns(graph);
    let window = days.min(x.len());
    let previous = x.len() >= days * 2;
    let mut out = json!({"from":if window>0{day(&x[x.len()-window])}else{Value::Null},"to":x.last().map(day),"series":{}});
    let mut grand = 0.0;
    for (label, values) in ys {
        let available = window.min(values.len());
        let current = &values[values.len() - available..];
        let before = if previous && values.len() >= days * 2 {
            Some(&values[values.len() - days * 2..values.len() - days])
        } else {
            None
        };
        let level = matches!(name, "growth_graph" | "followers_graph");
        let mut row = json!({});
        if level {
            row["start"] = json!(current.first());
            row["end"] = json!(current.last());
            row["change"] = json!(current.first().zip(current.last()).map(|(a, b)| b - a));
            if let Some(b) = before {
                row["previous_change"] = json!(b.last().zip(b.first()).map(|(b, a)| b - a));
            }
        } else {
            let total: f64 = current.iter().sum();
            grand += total;
            let old = before.map(|b| b.iter().sum::<f64>());
            row = json!({"total":total,"previous_total":old,"daily_average":if window>0{json!((total/window as f64*10.0).round()/10.0)}else{Value::Null},"change_percent":old.map(|n|percent(total,n))});
        }
        if !current.is_empty() {
            let top = current
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap();
            let low = current
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .unwrap();
            let offset = x.len() - available;
            row["peak"] = json!({"date":day(&x[offset+top.0]),"value":top.1});
            row["low"] = json!({"date":day(&x[offset+low.0]),"value":low.1});
        }
        out["series"][&label] = row;
    }
    if !matches!(name, "growth_graph" | "followers_graph")
        && let Some(rows) = out["series"].as_object_mut()
    {
        for row in rows.values_mut() {
            row["share_percent"] = if grand == 0.0 {
                Value::Null
            } else {
                json!((row["total"].as_f64().unwrap_or(0.0) * 1000.0 / grand).round() / 10.0)
            };
        }
    }
    if !previous {
        out["note"] = json!(format!(
            "Telegram sent {} days, too few for a previous {days}-day period.",
            x.len()
        ));
    }
    out
}
fn hours(graph: &Value) -> Value {
    let (x, series) = columns(graph);
    let Some((label, values)) = series.first() else {
        return json!({"status":"empty"});
    };
    let Some(top) = values.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
        return json!({"status":"empty"});
    };
    let low = values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let mut out = json!({"period":label,"busiest_hour":x.get(top.0),"quietest_hour":x.get(low.0),"busiest_hour_value":top.1,"note":"Hours are in UTC."});
    if let Some((previous, old)) = series.get(1) {
        out["previous_period"] = json!(previous);
        out["change_percent"] = percent(values.iter().sum(), old.iter().sum());
    }
    out
}
fn weekdays(graph: &Value, days: usize) -> Value {
    let (x, series) = columns(graph);
    let weeks = days.div_ceil(7).max(1);
    let mut totals = Vec::new();
    for (label, values) in series {
        totals.push((
            label,
            values[values.len().saturating_sub(weeks)..]
                .iter()
                .sum::<f64>(),
        ));
    }
    if totals.is_empty() {
        return json!({"status":"empty"});
    }
    let grand: f64 = totals.iter().map(|(_, v)| v).sum();
    let top = totals.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    let low = totals.iter().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    let mut out = json!({"weeks":weeks.min(x.len()),"busiest_weekday":top.0,"quietest_weekday":low.0,"totals":{},"share_percent":{}});
    for (label, value) in totals {
        out["totals"][&label] = json!(value);
        out["share_percent"][&label] = if grand == 0.0 {
            Value::Null
        } else {
            json!((value * 1000.0 / grand).round() / 10.0)
        };
    }
    out
}
pub(super) fn analyze(input: &Value, days: usize, include: bool) -> Value {
    let mut out = input.clone();
    let fields: Vec<_> = input
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    for name in fields {
        let v = &input[&name];
        if let (Some(c), Some(p)) = (v["current"].as_f64(), v["previous"].as_f64()) {
            out[&name]["change_percent"] = percent(c, p);
        } else if v["status"] == "loaded" {
            let mut analysis = match name.as_str() {
                "top_hours_graph" => hours(v),
                "weekdays_graph" => weekdays(v, days),
                _ => daily(&name, v, days),
            };
            if include {
                analysis["columns"] = v["columns"].clone();
            }
            out[&name] = analysis;
        }
    }
    out
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let p = a::peer(api, args, "chat_id").await?;
    if matches!(
        name,
        "get_fact_check" | "set_fact_check" | "delete_fact_check"
    ) {
        return match name {
            "get_fact_check" => {
                api.invoke(
                    "messages.getFactCheck",
                    json!({"peer":p,"msg_id":ids(a::required(args,"message_ids")?,100)?}),
                )
                .await
            }
            "set_fact_check" => {
                let text = a::text(args, "text")?;
                nonempty(text, 1024, "Fact check")?;
                api.invoke("messages.editFactCheck",json!({"peer":p,"msg_id":positive(a::required(args,"message_id")?)?,"text":txt(text)})).await
            }
            "delete_fact_check" => {
                api.invoke(
                    "messages.deleteFactCheck",
                    json!({"peer":p,"msg_id":positive(a::required(args,"message_id")?)?}),
                )
                .await
            }
            _ => bail!("Unknown fact-check operation"),
        };
    }
    let c = channel(&p)?;
    let analyze_stats = name == "analyze_chat_statistics";
    let days = a::integer(args, "days", 30)?;
    if analyze_stats {
        ensure!((1..=90).contains(&days), "days must be 1..=90");
    }
    let mut params = json!({"channel":c});
    let method = if let Some(mid) = args.get("message_id").filter(|v| !v.is_null()) {
        params["msg_id"] = json!(positive(mid)?);
        "stats.getMessageStats"
    } else {
        let result = api
            .invoke("channels.getChannels", json!({"id":[c]}))
            .await?;
        let entity = result["chats"]
            .as_array()
            .and_then(|v| v.first())
            .ok_or_else(|| anyhow::anyhow!("Channel not found"))?;
        if entity["broadcast"].as_bool() == Some(true) {
            "stats.getBroadcastStats"
        } else {
            ensure!(
                entity["megagroup"].as_bool() == Some(true),
                "Statistics are available only for a channel or supergroup"
            );
            "stats.getMegagroupStats"
        }
    };
    let mut result = api.invoke(method, params).await?;
    let include = if analyze_stats {
        true
    } else {
        a::boolean(args, "include_graph_data", false)?
    };
    resolve_graphs(
        api,
        &mut result,
        analyze_stats || a::boolean(args, "resolve_graphs", true)?,
        include,
    )
    .await?;
    if analyze_stats {
        return Ok(analyze(
            &result,
            days as usize,
            a::boolean(args, "include_series", false)?,
        ));
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_stats.rs"]
mod tests;
