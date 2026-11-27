//! Tool routing and local-API dispatch helpers split from `src/server.rs`.
use super::{
    Mcp, QR_WORKERS, access::AccessPolicy, approvals::SavedMessages,
    request_context::TrustedRequestContext,
};
use crate::{
    backend::Telegram,
    safeguard::{
        channels::{ChannelAvailability, ChannelOutcome, DialogElicitor},
        folders::{FolderGrants, FolderScope, tool_paths},
    },
    state::{FileAccess, ScopedPathGrant},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(PartialEq, Eq)]
pub(super) struct FolderPaths(Vec<ScopedPathGrant>);

impl FolderPaths {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn bind(&self, args: &mut Value) {
        if !self.is_empty() {
            args["_approved_file_paths"] =
                json!(self.0.iter().map(|grant| &grant.path).collect::<Vec<_>>());
        }
    }

    pub(super) fn for_call(mcp: &Mcp, name: &str, args: &Value) -> Result<Self> {
        let state = &mcp.runtime.state;
        let store = FolderGrants::new(state, state.installation_root())?;
        let mut paths = Vec::new();
        for (raw, scope) in tool_paths(name, args) {
            crate::state::validate_path_syntax(raw)?;
            let path = store
                .resolve_for_call(raw)
                .map_err(|_| anyhow::anyhow!("The requested path is protected or unavailable"))?;
            let allow_descendants = scope == FolderScope::Write && path.is_dir();
            let access = if scope == FolderScope::Read {
                FileAccess::Read
            } else {
                FileAccess::Write
            };
            if access == FileAccess::Write
                && !allow_descendants
                && matches!(
                    name,
                    "download_media" | "download_rich_media" | "save_disappearing_media"
                )
            {
                let mut sanitized = crate::state::sanitize_download_path(&path)?;
                if sanitized
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("msp"))
                {
                    sanitized.set_extension("bin");
                }
                let sanitized = store.resolve_for_call(&sanitized).map_err(|_| {
                    anyhow::anyhow!("The requested path is protected or unavailable")
                })?;
                if sanitized != path {
                    paths.push(ScopedPathGrant {
                        path: sanitized,
                        access,
                        allow_descendants: false,
                    });
                }
            }
            paths.push(ScopedPathGrant {
                path,
                access,
                allow_descendants,
            });
        }
        Ok(Self(paths))
    }

    pub(super) fn grants(
        &self,
        mcp: &Mcp,
        account: &str,
        request_id: &str,
        approved: bool,
    ) -> Result<Vec<ScopedPathGrant>> {
        let guard = &mcp.runtime.guard;
        let mut grants = Vec::new();
        for grant in &self.0 {
            let scope = match grant.access {
                FileAccess::Read => FolderScope::Read,
                FileAccess::Write => FolderScope::Write,
            };
            let path = grant.path.as_path();
            let durable = guard.authorize_folder("", account, path, scope, None)?;
            let authorized = if durable.is_some() {
                durable
            } else {
                let outcome = if approved {
                    ChannelOutcome::ApprovedOnce
                } else {
                    ChannelOutcome::Unsupported
                };
                let once =
                    guard.temporary_folder_access(request_id, account, &[path], scope, outcome)?;
                guard.authorize_folder(request_id, account, path, scope, once.as_ref())?
            };
            ensure!(
                authorized.as_deref() == Some(path),
                "The path is outside configured file roots or changed while waiting; no file operation was dispatched"
            );
            // Validate the same canonical path once more at the operation's filesystem boundary.
            grants.push(grant.clone());
        }
        Ok(grants)
    }
}

pub(super) struct LocalApi(pub(super) Mcp);

#[async_trait::async_trait]
impl Telegram for LocalApi {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        match method {
            "local.accounts" => {
                let connected: Vec<(String, Arc<dyn Telegram>)> = self
                    .0
                    .runtime
                    .accounts
                    .lock()
                    .await
                    .iter()
                    .map(|(label, slot)| (label.clone(), Arc::clone(&slot.api)))
                    .collect();
                let snapshot = crate::backend::live_accounts_snapshot(&connected).await?;
                for entry in snapshot["accounts"].as_array().into_iter().flatten() {
                    if let Some(label) = entry["label"].as_str() {
                        let availability = match entry["status"].as_str() {
                            Some("connected")
                                if super::approvals::positive_id(&entry["user_id"]).is_some() =>
                            {
                                ChannelAvailability::Available
                            }
                            Some("connected" | "unavailable") => ChannelAvailability::Unavailable,
                            _ => ChannelAvailability::Unknown,
                        };
                        self.0
                            .runtime
                            .guard
                            .observe_approval_account(label, availability);
                    }
                }
                Ok(snapshot)
            }
            "local.session_health" => {
                let key = params["config_key"]
                    .as_str()
                    .context("config_key is required")?;
                let label = if key == "TELEGRAM_SESSION_STRINGS" {
                    "default".to_owned()
                } else {
                    crate::account_manager::label_for(key)?
                };
                let account = self.0.runtime.accounts.lock().await.get(&label).cloned();
                if let Some(slot) = account {
                    let health = slot.api.invoke(method, params).await;
                    let availability =
                        match health.as_ref().ok().and_then(|v| v["authorized"].as_bool()) {
                            Some(true) => ChannelAvailability::Available,
                            Some(false) => ChannelAvailability::Unavailable,
                            None => ChannelAvailability::Unknown,
                        };
                    self.0
                        .runtime
                        .guard
                        .observe_approval_account(&label, availability);
                    health
                } else {
                    self.0
                        .runtime
                        .guard
                        .observe_approval_account(&label, ChannelAvailability::Unknown);
                    Ok(
                        json!({"status":"unknown","authorized":null,"account":label,"reason":"No live owner exists for this account"}),
                    )
                }
            }
            _ => bail!("This local operation requires an active account"),
        }
    }
    async fn resolve_peer(&self, _input: &Value) -> Result<Value> {
        bail!("This operation requires an active Telegram account")
    }
}

pub(super) fn is_local_only(name: &str, args: &Value) -> bool {
    matches!(
        name,
        "list_accounts"
            | "check_account_session"
            | "get_file_roots_status"
            | "list_contact_aliases"
            | "delete_contact_alias"
            | "get_ghost_mode"
            | "set_ghost_mode"
            | "safeguard_status"
            | "revoke_always_approval"
            | "incoming_feed_status"
            | "disable_incoming_feed"
            | "add_proxies"
            | "list_proxies"
            | "remove_proxies"
            | "test_proxies"
            | "prepare_upload"
            | "wait_for_events"
            | "export_status"
            | "cancel_export"
            | "get_invite_link_qr"
            | "transcribe_audio"
    ) && !(name == "add_proxies" && args.get("source").is_some_and(|v| !v.is_null()))
        && !(matches!(name, "get_ghost_mode" | "set_ghost_mode")
            && args.get("chat_id").is_some_and(|chat| !chat.is_null()))
}

pub(super) fn multi_account_defaults_to_default(name: &str, args: &Value) -> bool {
    matches!(
        name,
        "list_accounts"
            | "check_account_session"
            | "get_file_roots_status"
            | "safeguard_status"
            | "list_proxies"
            | "remove_proxies"
            | "test_proxies"
            | "get_ghost_mode"
            | "incoming_feed_status"
            | "prepare_upload"
            | "wait_for_events"
            | "export_status"
            | "cancel_export"
            | "get_invite_link_qr"
    ) || (name == "add_proxies" && args.get("source").is_none_or(Value::is_null))
}

pub(super) async fn invoke_operation(
    mcp: &Mcp,
    api: &dyn Telegram,
    name: &str,
    args: &Value,
    policy: &AccessPolicy,
    request_ctx: &TrustedRequestContext,
) -> Result<Value> {
    if name == "wait_for_events" {
        crate::tools::local::upstream_events::wait_for_events_with_state(
            api,
            &mcp.runtime.state,
            args,
            &mcp.runtime.policy,
        )
        .await
    } else if matches!(name, "wait_for_new_message" | "wait_for_settled_message") {
        crate::tools::local::wait_incoming_with_policy(api, &mcp.runtime.state, name, args, policy)
            .await
    } else if name == "safeguard_status" {
        let scoped = args.get("account").and_then(Value::as_str);
        let slots: Vec<_> = mcp
            .runtime
            .accounts
            .lock()
            .await
            .iter()
            .filter(|(label, _)| scoped.is_none_or(|scope| scope == label.as_str()))
            .map(|(label, slot)| (label.clone(), Arc::clone(&slot.api)))
            .collect();
        let guard = &mcp.runtime.guard;
        let (roots, observed) = tokio::join!(
            request_ctx.fetch_client_roots(),
            futures_util::future::join_all(slots.iter().map(|(label, api)| async move {
                (
                    label,
                    SavedMessages::observe(guard, label, api.as_ref())
                        .await
                        .availability(),
                )
            })),
        );
        let mut status = guard.status_for_account(scoped);
        status["channels"]["dialog"] = json!(if request_ctx
            .verified_owner(scoped.unwrap_or_default())
            .is_some()
            && request_ctx.supports_ordinary_elicitation()
        {
            ChannelAvailability::Available
        } else {
            ChannelAvailability::Unavailable
        });
        for (label, availability) in observed {
            status["channels"]["saved_messages"][label.as_str()] = json!(availability);
        }
        status["folders"]["client_root_count"] = json!(roots.as_ref().map(Vec::len));
        Ok(status)
    } else if name == "get_file_roots_status" {
        let client_roots = request_ctx.fetch_client_roots().await;
        Ok(crate::tools::local::roots::get_file_roots_status(
            &mcp.runtime.state,
            client_roots,
        ))
    } else if matches!(
        name,
        "download_media" | "download_rich_media" | "save_disappearing_media"
    ) {
        crate::tools::messaging::media::downloads::run_with_state(
            api,
            &mcp.runtime.state,
            name,
            args,
        )
        .await
    } else if name == "transcribe_audio" {
        crate::tools::messaging::transcribe::transcribe_audio_with_state(&mcp.runtime.state, args)
            .await
    } else if name == "get_invite_link_qr" {
        let permit = QR_WORKERS
            .acquire()
            .await
            .context("QR workers are unavailable")?;
        let state = Arc::clone(&mcp.runtime.state);
        let args = args.clone();
        tokio::task::spawn_blocking(move || {
            // The worker owns both admission and anchored staging until
            // rendering/publication finish, even if its caller leaves.
            let _permit = permit;
            crate::tools::identity::invite_link_qr_in(&state, &args)
        })
        .await
        .context("QR worker ended before returning its complete result")?
    } else {
        crate::tools::call(api, name, args).await
    }
}
