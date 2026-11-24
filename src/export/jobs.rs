use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

#[derive(Debug)]
pub(crate) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Export cancelled")
    }
}
impl std::error::Error for Cancelled {}

#[derive(Clone)]
struct Job {
    record: Value,
    cancel: watch::Sender<bool>,
}

static JOBS: OnceLock<Mutex<BTreeMap<String, Job>>> = OnceLock::new();

fn jobs() -> &'static Mutex<BTreeMap<String, Job>> {
    JOBS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn start(
    api: Arc<dyn crate::backend::Telegram>,
    kind: &str,
    args: Value,
    mut settings: super::settings::Settings,
) -> Result<Value> {
    let id = format!(
        "export-{}",
        crate::args::random_id()
            .as_str()
            .context("Random identifier was not a string")?
    );
    let (cancel, receiver) = watch::channel(false);
    let reservation = settings.reserve()?;
    let record = json!({"job_id":id,"kind":kind,"chat_id":args["chat_id"],"folder":settings.path,
        "status":"running","messages_so_far":0,"files_so_far":0,"started_at":chrono::Utc::now().to_rfc3339()});
    {
        let mut all = jobs()
            .lock()
            .map_err(|_| anyhow::anyhow!("Export job registry is unavailable"))?;
        if all.len() >= 256 {
            all.retain(|_, job| job.record["status"] == "running");
            if all.len() >= 256 {
                bail!("Too many active export jobs");
            }
        }
        all.insert(
            id.clone(),
            Job {
                record: record.clone(),
                cancel,
            },
        );
    }
    let kind = kind.to_owned();
    tokio::spawn(async move {
        let result =
            super::run_inner(api.as_ref(), &kind, &args, &settings, receiver, &id, true).await;
        drop(reservation);
        if let Ok(mut all) = jobs().lock()
            && let Some(job) = all.get_mut(&id)
        {
            job.record["status"] = if result.is_ok() {
                "done"
            } else if result.as_ref().is_err_and(|error| error.is::<Cancelled>()) {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            job.record["finished_at"] = chrono::Utc::now().to_rfc3339().into();
            match result {
                Ok(value) => job.record["result"] = value,
                Err(error) => {
                    // Detailed transport errors can contain peer data. The
                    // private job record exposes only our bounded context.
                    job.record["error"] = error
                        .to_string()
                        .chars()
                        .take(400)
                        .collect::<String>()
                        .into();
                }
            }
        }
    });
    Ok(record)
}

pub fn progress(id: &str, messages: usize, files: usize) {
    if let Ok(mut all) = jobs().lock()
        && let Some(job) = all.get_mut(id)
    {
        job.record["messages_so_far"] = messages.into();
        job.record["files_so_far"] = files.into();
    }
}

pub fn status(args: &Value) -> Result<Value> {
    let all = jobs()
        .lock()
        .map_err(|_| anyhow::anyhow!("Export job registry is unavailable"))?;
    match args["job_id"].as_str().filter(|id| !id.is_empty()) {
        Some(id) => all
            .get(id)
            .map(|job| job.record.clone())
            .with_context(|| format!("No export {id}; export_status without job_id lists them")),
        None => Ok(all.values().map(|job| job.record.clone()).collect()),
    }
}

pub fn cancel(args: &Value) -> Result<Value> {
    let id = crate::args::text(args, "job_id")?;
    let all = jobs()
        .lock()
        .map_err(|_| anyhow::anyhow!("Export job registry is unavailable"))?;
    let cancelled = all.get(id).is_some_and(|job| {
        job.record["status"] == "running" && !*job.cancel.borrow() && job.cancel.send(true).is_ok()
    });
    Ok(json!({"job_id":id,"cancelled":cancelled}))
}

pub fn start_secret<F, Fut>(chat_id: Value, folder: String, work: F) -> Result<Value>
where
    F: FnOnce(watch::Receiver<bool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(Vec<std::path::PathBuf>, usize)>> + Send + 'static,
{
    let id = format!(
        "export-{}",
        crate::args::random_id()
            .as_str()
            .context("Random identifier was not a string")?
    );
    let (cancel, receiver) = watch::channel(false);
    let record = json!({
        "job_id": id,
        "kind": "secret_chat",
        "chat_id": chat_id,
        "folder": folder,
        "status": "running",
        "messages_so_far": 0,
        "files_so_far": 0,
        "started_at": chrono::Utc::now().to_rfc3339(),
    });
    {
        let mut all = jobs()
            .lock()
            .map_err(|_| anyhow::anyhow!("Export job registry is unavailable"))?;
        if all.len() >= 128 {
            if let Some(finished) = all
                .iter()
                .find(|(_, job)| job.record["status"] != "running")
                .map(|(k, _)| k.clone())
            {
                all.remove(&finished);
            } else {
                bail!("Too many active export jobs");
            }
        }
        all.insert(
            id.clone(),
            Job {
                record: record.clone(),
                cancel,
            },
        );
    }
    let job_id = id.clone();
    tokio::spawn(async move {
        let result = work(receiver).await;
        if let Ok(mut all) = jobs().lock()
            && let Some(job) = all.get_mut(&job_id)
        {
            job.record["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
            match result {
                Ok((paths, messages)) => {
                    job.record["status"] = json!("done");
                    job.record["messages_so_far"] = json!(messages);
                    job.record["files_so_far"] = json!(paths.len());
                    if let Some(first) = paths.first() {
                        job.record["path"] = json!(first);
                    }
                    job.record["paths"] = json!(paths);
                }
                Err(error) if error.is::<Cancelled>() => {
                    job.record["status"] = json!("cancelled");
                }
                Err(error) => {
                    job.record["status"] = json!("failed");
                    job.record["error"] = json!(error.to_string());
                }
            }
        }
    });
    Ok(record)
}
