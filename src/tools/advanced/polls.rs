use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use base64::Engine;
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if name == "vote_poll" {
        return Some(vote_alias(api, args).await);
    }

    if !matches!(
        name,
        "create_poll"
            | "get_poll_results"
            | "vote_in_poll"
            | "get_poll_voters"
            | "close_poll"
            | "add_poll_option"
            | "delete_poll_option"
            | "list_unread_poll_votes"
            | "mark_poll_votes_read"
            | "get_poll_statistics"
            | "reply_to_part"
            | "get_part_link"
    ) {
        return None;
    }
    Some(if name == "create_poll" {
        create(api, args).await
    } else {
        run(api, name, args).await
    })
}
fn option(poll: &Value, index: i64) -> Result<Value> {
    ensure!(index >= 0, "Option index must be nonnegative");
    poll["answers"]
        .as_array()
        .and_then(|v| v.get(index as usize))
        .and_then(|v| v.get("option"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Poll option does not exist; read get_poll_results first"))
}
fn describe(poll: &Value, result: &Value) -> Value {
    let tallies = result["results"].as_array().cloned().unwrap_or_default();
    let total = result["total_voters"].as_i64();
    let answers:Vec<_>=poll["answers"].as_array().into_iter().flatten().enumerate().map(|(i,v)|{
        let count=tallies.iter().find(|r|r["option"]==v["option"]);
        let mut out=json!({"index":i,"text":v["text"]["text"],"voters":count.and_then(|r|r["voters"].as_i64()),"chosen_by_this_account":count.is_some_and(|r|r["chosen"].as_bool()==Some(true))});
        if let Some(count) = count {
            if let (Some(v), Some(t)) = (count["voters"].as_i64(), total) && t > 0 {
                out["share_percent"] = json!((v as f64 * 1000.0 / t as f64).round() / 10.0);
            }
            if count["correct"].as_bool() == Some(true) {
                out["correct"] = json!(true);
            }
        }
        if let Some(media)=v.get("media"){out["attachment"]=media.clone();}out
    }).collect();
    let mut out = json!({"question":poll["question"]["text"],"options":answers,"total_voters":total,"can_view_stats":result["can_view_stats"].as_bool().unwrap_or(false)});
    for flag in [
        "closed",
        "quiz",
        "multiple_choice",
        "public_voters",
        "open_answers",
        "revoting_disabled",
        "shuffle_answers",
        "hide_results_until_close",
        "subscribers_only",
    ] {
        out[flag] = json!(poll[flag].as_bool().unwrap_or(false));
    }
    for field in ["close_date", "close_period", "countries_iso2"] {
        if let Some(v) = poll.get(field) {
            out[field] = v.clone();
        }
    }
    if let Some(s) = result.get("solution") {
        out["solution"] = s.clone();
    }
    let chosen: Vec<_> = answers
        .iter()
        .filter(|v| v["chosen_by_this_account"] == true)
        .map(|v| v["index"].clone())
        .collect();
    out["your_votes"] = json!(chosen);
    if poll["quiz"].as_bool() == Some(true) {
        let correct: Vec<_> = answers
            .iter()
            .filter(|v| v["correct"] == true)
            .map(|v| v["index"].clone())
            .collect();
        out["correct_option_index"] = correct.first().cloned().unwrap_or(Value::Null);
        out["correct_option_indexes"] = json!(correct);
        if correct.is_empty() {
            out["correct_option_note"] = json!(
                "Telegram withholds the correct answer until this account has answered the quiz."
            );
        }
    }
    out
}
fn deadline(when: i64) -> Result<()> {
    let seconds = when
        .checked_sub(chrono::Utc::now().timestamp())
        .ok_or_else(|| anyhow::anyhow!("close_date is outside the supported timestamp range"))?;
    ensure!(
        (7..=2_628_000).contains(&seconds),
        "close_date must be 7 seconds to about 30 days in the future"
    );
    Ok(())
}
async fn create(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let question = parsed(a::text(args, "question")?, args)?;
    nonempty(
        question["text"].as_str().unwrap_or_default(),
        255,
        "Poll question",
    )?;
    let raw = a::required(args, "options")?
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("options must be a list"))?;
    ensure!(
        raw.len() >= 2 && raw.len() <= 100,
        "A poll needs at least two options"
    );
    let mut options = Vec::new();
    for v in raw {
        let s = v
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Poll options must be strings"))?;
        let option = parsed(s, args)?;
        nonempty(
            option["text"].as_str().unwrap_or_default(),
            100,
            "Poll option",
        )?;
        options.push(option);
    }
    let quiz = a::boolean(args, "quiz_mode", false)?;
    let multiple = a::boolean(args, "multiple_choice", false)?;
    let adding = a::boolean(args, "allow_adding_options", false)?;
    let revote = args
        .get("allow_revoting")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| anyhow::anyhow!("allow_revoting must be boolean"))
        })
        .transpose()?;
    let public = args
        .get("public_votes")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| anyhow::anyhow!("public_votes must be boolean"))
        })
        .transpose()?;
    let one = args.get("correct_option_index").filter(|v| !v.is_null());
    let many = args.get("correct_option_indexes").filter(|v| !v.is_null());
    ensure!(
        one.is_none() || many.is_none(),
        "Pass correct_option_index or correct_option_indexes, not both"
    );
    let correct: Vec<i64> = if let Some(v) = one {
        vec![id(v)?]
    } else if let Some(v) = many {
        v.as_array()
            .ok_or_else(|| anyhow::anyhow!("correct_option_indexes must be a list"))?
            .iter()
            .map(id)
            .collect::<Result<_>>()?
    } else {
        vec![]
    };
    unique(&correct)?;
    ensure!(
        !quiz || (!correct.is_empty() && revote != Some(true) && !adding),
        "A quiz needs a correct answer and permits neither revoting nor adding options"
    );
    ensure!(
        quiz || correct.is_empty(),
        "Correct answers only apply to quizzes"
    );
    ensure!(
        correct
            .iter()
            .all(|i| *i >= 0 && (*i as usize) < options.len()),
        "Correct option index is outside the options"
    );
    ensure!(
        !adding || public != Some(false),
        "Adding options requires public votes"
    );
    let solution = parsed(string(args, "explanation", "")?, args)?;
    let explanation = solution["text"].as_str().unwrap_or_default();
    ensure!(
        explanation.encode_utf16().count() <= 200,
        "Quiz explanation exceeds 200 UTF-16 units"
    );
    ensure!(
        quiz || (explanation.is_empty() && args.get("explanation_file").is_none_or(Value::is_null)),
        "Explanations only apply to quizzes"
    );
    let close = args
        .get("close_date")
        .filter(|v| !v.is_null())
        .map(|_| timestamp(args, "close_date"))
        .transpose()?;
    let duration = args
        .get("duration_seconds")
        .filter(|v| !v.is_null())
        .map(id)
        .transpose()?;
    ensure!(
        close.is_none() || duration.is_none(),
        "Pass duration_seconds or close_date, not both"
    );
    if let Some(d) = duration {
        ensure!(
            (5..=2_628_000).contains(&d),
            "duration_seconds must be 5..=2628000"
        );
    }
    if let Some(c) = close {
        deadline(c)?;
    }
    let country = args.get("countries").filter(|v| !v.is_null());
    let mut countries = Vec::new();
    if let Some(v) = country {
        for c in v
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("countries must be a list"))?
        {
            let c = c
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Country codes must be strings"))?;
            ensure!(
                c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic()),
                "Countries must use two-letter ISO country codes"
            );
            countries.push(c.to_ascii_uppercase());
        }
    }
    let files = args
        .get("option_files")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_array()
                .ok_or_else(|| anyhow::anyhow!("option_files must be a list"))
        })
        .transpose()?;
    if let Some(f) = files {
        ensure!(
            f.len() == options.len(),
            "option_files needs exactly one entry per option"
        );
    }
    let limits = app_limits(api).await;
    let max = limits["poll_answers_max"].as_i64().unwrap_or(12);
    ensure!(
        options.len() <= max as usize,
        "Telegram allows at most {max} options"
    );
    let p = a::peer(api, args, "chat_id").await?;
    let mut broadcast = false;
    if p["_"] == "inputPeerChannel" {
        let chats = api
            .invoke("channels.getChannels", json!({"id":[channel(&p)?]}))
            .await?;
        broadcast = chats["chats"]
            .as_array()
            .and_then(|v| v.first())
            .is_some_and(|v| v["broadcast"].as_bool() == Some(true));
    }
    ensure!(
        !broadcast || (public != Some(true) && !adding),
        "Broadcast-channel polls are anonymous and cannot take added options"
    );
    let mut answers = Vec::new();
    for (i, s) in options.iter().enumerate() {
        let mut answer = json!({"_":"inputPollAnswer","text":s});
        if let Some(path) = files.and_then(|v| v[i].as_str()) {
            answer["media"] = attachment(api, &p, path).await?;
        }
        answers.push(answer);
    }
    let mut poll = json!({"_":"poll","id":a::random_id(),"hash":"0","question":question,"answers":answers,"quiz":quiz,"multiple_choice":multiple||correct.len()>1,"public_voters":!broadcast&&public.unwrap_or(true),"open_answers":adding,"revoting_disabled":quiz||revote==Some(false),"shuffle_answers":a::boolean(args,"shuffle_options",false)?,"hide_results_until_close":a::boolean(args,"hide_results_until_close",false)?,"subscribers_only":a::boolean(args,"members_only",false)?});
    if let Some(c) = close {
        deadline(c)?;
        poll["close_date"] = json!(c);
    }
    if let Some(d) = duration {
        poll["close_period"] = json!(d);
    }
    if !countries.is_empty() {
        poll["countries_iso2"] = json!(countries);
    }
    let mut media = json!({"_":"inputMediaPoll","poll":poll});
    if quiz {
        media["correct_answers"] = json!(correct);
        if !explanation.is_empty() {
            media["solution"] = solution["text"].clone();
            media["solution_entities"] = solution["entities"].clone();
        }
    }
    for (key, to) in [
        ("description_file", "attached_media"),
        ("explanation_file", "solution_media"),
    ] {
        if let Some(path) = args.get(key).and_then(Value::as_str) {
            media[to] = attachment(api, &p, path).await?;
        }
    }
    if let Some(c) = close {
        deadline(c)?;
    }
    let caption = parsed(string(args, "description", "")?, args)?;
    api.invoke("messages.sendMedia",json!({"peer":p,"media":media,"message":caption["text"],"entities":caption["entities"],"random_id":a::random_id()})).await
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let p = a::peer(api, args, "chat_id").await?;
    if name == "list_unread_poll_votes" {
        return api.invoke("messages.getUnreadPollVotes",json!({"peer":p,"offset_id":0,"add_offset":0,"limit":a::limit(args,"limit",50,200)?,"max_id":0,"min_id":0})).await;
    }
    if name == "mark_poll_votes_read" {
        return api
            .invoke("messages.readPollVotes", json!({"peer":p}))
            .await;
    }
    let mid = positive(a::required(args, "message_id")?)?;
    let msg = message(api, &p, mid).await?;
    if matches!(name, "reply_to_part" | "get_part_link") {
        return part(api, name, args, &p, mid, &msg).await;
    }
    let poll = &msg["media"]["poll"];
    ensure!(poll["_"] == "poll", "Message is not a poll");
    let mut full_results = msg["media"]["results"].clone();
    if full_results["min"].as_bool() == Some(true) {
        let fresh = api
            .invoke(
                "messages.getPollResults",
                json!({"peer":p,"msg_id":mid,"poll_hash":"0"}),
            )
            .await?;
        if let Some(r) = fresh["updates"].as_array().and_then(|v| {
            v.iter()
                .find_map(|v| v.get("results").filter(|r| r["results"].is_array()))
        }) {
            full_results = r.clone();
        }
    }
    let results = &full_results;
    match name{
        "get_poll_results"=>Ok(describe(poll,results)),
        "vote_in_poll"=>{
            ensure!(!poll["closed"].as_bool().unwrap_or(false),"Poll is closed");
            let raw=a::required(args,"option_indexes")?;let values=raw.as_array().cloned().unwrap_or_else(||vec![raw.clone()]);let mut seen=std::collections::HashSet::new();let indexes:Vec<_>=values.iter().map(id).collect::<Result<Vec<_>>>()?.into_iter().filter(|n|seen.insert(*n)).collect();
            ensure!(indexes.len()<=1||poll["multiple_choice"].as_bool()==Some(true),"This poll permits only one option");
            let bytes:Vec<_>=indexes.into_iter().map(|i|option(poll,i)).collect::<Result<_>>()?;
            api.invoke("messages.sendVote",json!({"peer":p,"msg_id":mid,"options":bytes})).await
        }
        "get_poll_voters"=>{
            ensure!(poll["public_voters"].as_bool()==Some(true),"Voter identities are unavailable for an anonymous poll");
            let mut params=json!({"peer":p,"id":mid,"limit":a::limit(args,"limit",100,200)?});
            if let Some(i)=args.get("option_index").filter(|v|!v.is_null()){params["option"]=option(poll,id(i)?)?;}
            copy_optional(args,&mut params,&["offset"]);api.invoke("messages.getPollVotes",params).await
        }
        "close_poll"=>{
            if poll["closed"].as_bool()==Some(true){return Ok(json!({"changed":false,"closed":true}));}
            let mut closed=poll.clone();closed["closed"]=json!(true);
            let mut media=json!({"_":"inputMediaPoll","poll":closed});
            let correct:Vec<_>=poll["answers"].as_array().into_iter().flatten().enumerate().filter(|(_,a)|results["results"].as_array().is_some_and(|v|v.iter().any(|r|r["option"]==a["option"]&&r["correct"].as_bool()==Some(true)))).map(|(i,_)|i).collect();
            if !correct.is_empty(){media["correct_answers"]=json!(correct);}
            if let Some(s)=results.get("solution"){media["solution"]=s.clone();media["solution_entities"]=results.get("solution_entities").cloned().unwrap_or(json!([]));}
            for(key,from)in [("solution_media",&results["solution_media"]),("attached_media",&msg["media"]["attached_media"]) ]{if !from.is_null(){media[key]=as_input_media(from)?;}}
            api.invoke("messages.editMessage",json!({"peer":p,"id":mid,"media":media})).await
        }
        "add_poll_option"=>{
            ensure!(poll["open_answers"].as_bool()==Some(true)&&poll["closed"].as_bool()!=Some(true),"This poll does not accept added options");
            let body=a::text(args,"text")?;nonempty(body,100,"Poll option")?;
            let mut answer=json!({"_":"inputPollAnswer","text":parsed(body,args)?});
            if let Some(path)=args.get("file_path").and_then(Value::as_str){answer["media"]=attachment(api,&p,path).await?;}
            api.invoke("messages.addPollAnswer",json!({"peer":p,"msg_id":mid,"answer":answer})).await
        }
        "delete_poll_option"=>api.invoke("messages.deletePollAnswer",json!({"peer":p,"msg_id":mid,"option":option(poll,a::integer(args,"option_index",-1)?)?})).await,
        "get_poll_statistics"=>{
            ensure!(results["can_view_stats"].as_bool()==Some(true),"Telegram does not offer statistics for this poll to this account");
            let mut result=api.invoke("stats.getPollStats",json!({"peer":p,"msg_id":mid})).await?;
            super::stats::resolve_graphs(api,&mut result,true,a::boolean(args,"include_series",false)?).await?;Ok(result)
        }
        _=>bail!("Unknown poll operation"),
    }
}
async fn part(
    api: &dyn Telegram,
    name: &str,
    args: &Value,
    p: &Value,
    mid: i64,
    msg: &Value,
) -> Result<Value> {
    let index = args.get("option_index").filter(|v| !v.is_null());
    let task = args.get("task_id").filter(|v| !v.is_null());
    ensure!(
        index.is_some() != task.is_some(),
        "Name exactly one of option_index or task_id"
    );
    let mut reply = json!({"_":"inputReplyToMessage","reply_to_msg_id":mid});
    let query = if let Some(i) = index {
        ensure!(msg["media"]["poll"]["_"] == "poll", "Message is not a poll");
        let o = option(&msg["media"]["poll"], id(i)?)?;
        let bytes = o
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid poll option bytes"))?
            .iter()
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n <= 255)
                    .map(|n| n as u8)
                    .ok_or_else(|| anyhow::anyhow!("Invalid poll option bytes"))
            })
            .collect::<Result<Vec<_>>>()?;
        reply["poll_option"] = o;
        format!(
            "option={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        )
    } else {
        let tid = positive(task.unwrap())?;
        let todo = &msg["media"]["todo"];
        ensure!(
            todo["list"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| id(&v["id"]).ok() == Some(tid))),
            "Checklist task does not exist"
        );
        reply["todo_item_id"] = json!(tid);
        format!("task={tid}")
    };
    if name == "get_part_link" {
        let r = api
            .invoke(
                "channels.exportMessageLink",
                json!({"channel":channel(p)?,"id":mid}),
            )
            .await?;
        let link = r["link"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Telegram did not return a message link"))?;
        return Ok(json!({"link":format!("{link}{}{query}",if link.contains('?'){"&"}else{"?"})}));
    }
    let body = parsed(a::text(args, "text")?, args)?;
    api.invoke("messages.sendMessage",json!({"peer":p,"message":body["text"],"entities":body["entities"],"reply_to":reply,"random_id":a::random_id()})).await
}

async fn vote_alias(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let message = a::required(args, "message_id")?
        .as_i64()
        .filter(|n| (1..=i32::MAX as i64).contains(n))
        .ok_or_else(|| anyhow::anyhow!("message_id must be a positive 32-bit integer"))?;
    let values = a::required(args, "option_indices")?
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("option_indices must be a non-empty list of integers"))?;
    ensure!(
        !values.is_empty() && values.len() <= 100,
        "option_indices must contain 1..=100 integers"
    );
    let wanted: Vec<i64> = values
        .iter()
        .map(|v| {
            v.as_i64().filter(|n| *n >= 0).ok_or_else(|| {
                anyhow::anyhow!("option_indices must contain nonnegative strict integers")
            })
        })
        .collect::<Result<_>>()?;
    unique(&wanted)?;
    let mut translated = args.clone();
    translated
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Vote arguments must be an object"))?
        .remove("option_indices");
    translated["option_indexes"] = json!(wanted);
    translated["message_id"] = json!(message);
    run(api, "vote_in_poll", &translated).await
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_polls.rs"]
mod tests;
