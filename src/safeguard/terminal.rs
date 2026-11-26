use super::{request_fingerprint, unix_seconds};
use crate::state::State;
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::io::{IsTerminal, Write};
use std::sync::Arc;

/// This entry point belongs only to the standalone owner CLI, never to MCP tools.
pub fn approve_from_terminal(state: Arc<State>, fingerprint: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("Approval requires an interactive owner terminal; piped input is refused");
    }
    if fingerprint.len() != 64 || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Invalid approval request fingerprint");
    }
    let requests = state.read_json("approval-pending.json", json!({}))?;
    let request = requests
        .get(fingerprint)
        .context("Approval request does not exist")?;
    if request["expires"].as_u64().unwrap_or(0) < unix_seconds() {
        bail!("Approval request expired; request the operation again");
    }
    let tool = request["tool"]
        .as_str()
        .context("Invalid pending approval request")?;
    if request_fingerprint(tool, &request["arguments"])? != fingerprint {
        bail!("Approval request fingerprint does not match its contents");
    }
    let seals = super::sealed::SealedStore::new(Arc::clone(&state));
    let display = super::approval_details::prompt_summary(&seals.redact_value(request));
    eprintln!("Review this operation:\n{display}");
    eprint!(
        "Type APPROVE for this request once, ALWAYS for this tool/chat/account, or anything else to deny: "
    );
    std::io::stderr().flush()?;
    let mut reply = String::new();
    std::io::stdin().read_line(&mut reply)?;
    if !["APPROVE", "ALWAYS"].contains(&reply.trim()) {
        remove_pending(&state, fingerprint)?;
        bail!("Approval denied; no grant was written");
    }
    let always = reply.trim() == "ALWAYS";
    accept(&state, fingerprint, always, false)?;
    if always {
        eprintln!("Always approval saved for this tool, chat and account. Retry the operation.");
    } else {
        eprintln!("Approved once. Retry the exact operation within 60 seconds.");
    }
    Ok(())
}

pub(super) fn outcome(
    state: &State,
    fingerprint: &str,
    tool: &str,
    args: &serde_json::Value,
) -> Result<super::channels::ChannelOutcome> {
    use super::channels::ChannelOutcome;
    Ok(if has_grant(state, fingerprint, tool, args)? {
        ChannelOutcome::ApprovedOnce
    } else {
        ChannelOutcome::Unsupported
    })
}

pub(super) fn remove_pending(state: &State, fingerprint: &str) -> Result<()> {
    state.update_json("approval-pending.json", json!({}), |rows| {
        rows.as_object_mut()
            .context("Approval request record is invalid")?
            .remove(fingerprint);
        Ok(())
    })
}

pub(super) fn has_grant(
    state: &State,
    fingerprint: &str,
    tool: &str,
    args: &serde_json::Value,
) -> Result<bool> {
    if super::chat_target(args).is_some_and(|chat| {
        state.approval_is_granted(args["account"].as_str().unwrap_or("default"), tool, chat)
    }) {
        return Ok(true);
    }
    let grants = state.read_json("approval-grants.json", json!({}))?;
    Ok(grants[fingerprint]["expires"]
        .as_u64()
        .is_some_and(|expiry| expiry >= unix_seconds()))
}

pub(super) fn accept_bot(state: &State, fingerprint: &str, always: bool) -> Result<()> {
    accept(state, fingerprint, always, true)
}

fn accept(state: &State, fingerprint: &str, always: bool, remove_request: bool) -> Result<()> {
    let _lock = state.state_lock()?;
    let mut requests = state.read_json_unlocked("approval-pending.json", json!({}))?;
    let pending = requests
        .as_object_mut()
        .context("Approval request record is invalid")?;
    let request = pending
        .get(fingerprint)
        .context("Approval request is no longer pending")?;
    let now = unix_seconds();
    if request["expires"]
        .as_u64()
        .is_none_or(|expiry| expiry < now)
    {
        bail!("Approval request expired while being reviewed");
    }
    let tool = request["tool"]
        .as_str()
        .context("Invalid pending approval request")?;
    let args = &request["arguments"];
    if request_fingerprint(tool, args)? != fingerprint {
        bail!("Approval request fingerprint does not match its contents");
    }
    let (file, grants) = if always {
        let chat = super::chat_target(args).context("Always approval requires a specific chat")?;
        let account = args["account"].as_str().unwrap_or("default");
        let file = "always-approvals.json";
        let mut grants = state.read_json_unlocked(file, json!({"grants":[],"folders":[]}))?;
        crate::state::insert_approval(&mut grants, account, tool, chat)?;
        (file, grants)
    } else {
        let file = "approval-grants.json";
        let mut grants = state.read_json_unlocked(file, json!({}))?;
        grants
            .as_object_mut()
            .context("Approval grant record is invalid")?
            .insert(fingerprint.to_owned(), json!({"expires":now+60}));
        (file, grants)
    };
    if remove_request {
        // Remove the pending request first so an incomplete write fails closed.
        pending.remove(fingerprint);
        state.write_json_unlocked("approval-pending.json", &requests)?;
    }
    state.write_json_unlocked(file, &grants)
}
