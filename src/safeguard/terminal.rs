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
    eprintln!(
        "Review this exact operation:\n{}",
        serde_json::to_string_pretty(request)?
    );
    eprint!(
        "Type APPROVE for this request once, ALWAYS for this tool/chat/account, or anything else to deny: "
    );
    std::io::stderr().flush()?;
    let mut reply = String::new();
    std::io::stdin().read_line(&mut reply)?;
    if !["APPROVE", "ALWAYS"].contains(&reply.trim()) {
        bail!("Approval denied; no grant was written");
    }
    // Re-check the lifetime after the human review, which may take several minutes.
    if request["expires"].as_u64().unwrap_or(0) < unix_seconds() {
        bail!("Approval request expired while being reviewed");
    }
    if reply.trim() == "ALWAYS" {
        let account = request["arguments"]["account"]
            .as_str()
            .unwrap_or("default");
        let chat = request["arguments"]
            .get("chat_id")
            .or_else(|| request["arguments"].get("to_chat_id"))
            .filter(|chat| !chat.is_null())
            .context("Always approval requires a specific chat")?;
        state.grant_approval(account, tool, chat)?;
        eprintln!("Always approval saved for this tool, chat and account. Retry the operation.");
        return Ok(());
    }
    state.update_json("approval-grants.json", json!({}), |grants| {
        if !grants.is_object() {
            bail!("Approval grant record is invalid");
        }
        grants[fingerprint] = json!({"expires":unix_seconds()+60});
        Ok(())
    })?;
    eprintln!("Approved once. Retry the exact operation within 60 seconds.");
    Ok(())
}
