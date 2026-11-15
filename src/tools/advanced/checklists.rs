use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "send_checklist"
            | "add_checklist_tasks"
            | "set_checklist_tasks_done"
            | "remove_checklist_tasks"
            | "get_checklist"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
fn tasks(args: &Value) -> Result<Vec<String>> {
    let raw = a::required(args, "tasks")?
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("tasks must be a list of strings"))?;
    ensure!(
        !raw.is_empty() && raw.len() <= 1000,
        "A checklist needs at least one task and at most 1000 tasks"
    );
    raw.iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Every task must be a string"))?;
            nonempty(s, 4096, "Task")?;
            Ok(s.into())
        })
        .collect()
}
fn check(limits: &Value, title: Option<&str>, tasks: &[String], existing: usize) -> Result<()> {
    if let Some(n) = limits["todo_items_max"].as_i64() {
        ensure!(
            existing + tasks.len() <= n as usize,
            "Checklist exceeds Telegram's {n}-task limit"
        );
    }
    if let (Some(n), Some(title)) = (limits["todo_title_length_max"].as_i64(), title) {
        ensure!(
            title.chars().count() <= n as usize,
            "Checklist title exceeds Telegram's {n}-character limit"
        );
    }
    if let Some(n) = limits["todo_item_length_max"].as_i64() {
        ensure!(
            tasks.iter().all(|t| t.chars().count() <= n as usize),
            "Checklist task exceeds Telegram's {n}-character limit"
        );
    }
    Ok(())
}
fn items(values: &[String], first: i64) -> Vec<Value> {
    values
        .iter()
        .enumerate()
        .map(|(i, s)| json!({"_":"todoItem","id":first+i as i64,"title":txt(s)}))
        .collect()
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    if name == "send_checklist" {
        let title = a::text(args, "title")?;
        nonempty(title, 4096, "Checklist title")?;
        let task = tasks(args)?;
        check(&app_limits(api).await, Some(title), &task, 0)?;
        return api.invoke("messages.sendMedia",json!({"peer":a::peer(api,args,"chat_id").await?,"media":{"_":"inputMediaTodo","todo":{"_":"todoList","title":txt(title),"list":items(&task,1),"others_can_append":a::boolean(args,"others_can_add",false)?,"others_can_complete":a::boolean(args,"others_can_complete",false)?}},"message":"","random_id":a::random_id()})).await;
    }
    let p = a::peer(api, args, "chat_id").await?;
    let mid = positive(a::required(args, "message_id")?)?;
    let msg = message(api, &p, mid).await?;
    let media = &msg["media"];
    ensure!(
        media["_"] == "messageMediaToDo",
        "Message is not a checklist"
    );
    let todo = &media["todo"];
    let list = todo["list"].as_array().cloned().unwrap_or_default();
    let held: Vec<_> = list.iter().filter_map(|v| id(&v["id"]).ok()).collect();
    match name {
        "get_checklist" => {
            let completed = media["completions"].as_array().cloned().unwrap_or_default();
            let rows:Vec<_>=list.iter().map(|v|{let c=completed.iter().find(|c|c["id"]==v["id"]);json!({"id":v["id"],"text":v["title"]["text"],"done":c.is_some(),"completed_by":c.map(|c|c["completed_by"].clone()),"completed_at":c.map(|c|c["date"].clone())})}).collect();
            Ok(
                json!({"records":rows,"title":todo["title"]["text"],"message_id":mid,"others_can_add":todo["others_can_append"].as_bool().unwrap_or(false),"others_can_complete":todo["others_can_complete"].as_bool().unwrap_or(false),"done_count":completed.len(),"task_count":rows.len()}),
            )
        }
        "add_checklist_tasks" => {
            let task = tasks(args)?;
            check(&app_limits(api).await, None, &task, list.len())?;
            let first = held.iter().max().copied().unwrap_or(0) + 1;
            ensure!(
                first + task.len() as i64 <= i32::MAX as i64,
                "Checklist task IDs exhausted"
            );
            api.invoke(
                "messages.appendTodoList",
                json!({"peer":p,"msg_id":mid,"list":items(&task,first)}),
            )
            .await
        }
        "set_checklist_tasks_done" => {
            let done = match args.get("done").filter(|v| !v.is_null()) {
                Some(v) => ids(v, 1000)?,
                None => vec![],
            };
            let undone = match args.get("undone").filter(|v| !v.is_null()) {
                Some(v) => ids(v, 1000)?,
                None => vec![],
            };
            ensure!(
                !done.is_empty() || !undone.is_empty(),
                "Give done or undone task IDs"
            );
            unique(&done)?;
            unique(&undone)?;
            ensure!(
                done.iter().chain(&undone).all(|v| held.contains(v)),
                "Task ID does not exist in this checklist"
            );
            ensure!(
                !done.iter().any(|v| undone.contains(v)),
                "A task cannot be both done and undone"
            );
            api.invoke(
                "messages.toggleTodoCompleted",
                json!({"peer":p,"msg_id":mid,"completed":done,"incompleted":undone}),
            )
            .await
        }
        "remove_checklist_tasks" => {
            let drop = ids(a::required(args, "task_ids")?, 1000)?;
            unique(&drop)?;
            ensure!(
                drop.iter().all(|v| held.contains(v)),
                "Task ID does not exist in this checklist"
            );
            let kept: Vec<_> = list
                .into_iter()
                .filter(|v| id(&v["id"]).is_ok_and(|n| !drop.contains(&n)))
                .collect();
            ensure!(
                !kept.is_empty(),
                "Removing every task is not supported; delete the message instead"
            );
            let mut updated = todo.clone();
            updated["list"] = json!(kept);
            let latest = message(api, &p, mid).await?;
            ensure!(
                latest["media"]["todo"] == *todo,
                "Checklist changed while preparing removal; read the checklist and retry"
            );
            let result = api
                .invoke(
                    "messages.editMessage",
                    json!({"peer":p,"id":mid,"media":{"_":"inputMediaTodo","todo":updated}}),
                )
                .await?;
            Ok(
                json!({"updates":result,"concurrency":"Optimistic snapshot check only; Telegram has no atomic checklist edit. Concurrent changes after the check may be overwritten."}),
            )
        }
        _ => bail!("Unknown checklist operation"),
    }
}
