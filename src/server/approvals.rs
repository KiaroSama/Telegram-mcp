//! Owner approval over the initiating MCP peer or the acting Telegram client.
use super::{Mcp, request_context::TrustedRequestContext};
use crate::{
    backend::Telegram,
    safeguard::{
        Safeguard,
        channels::{
            ApprovalPrompt, ApprovalRoutes, ChannelAvailability, ChannelOutcome, DialogElicitor,
            SavedMessagesTransport, SavedReply, dialog_available,
        },
    },
};
use anyhow::Result;
use rmcp::{
    model::{ElicitRequestParams, ElicitationAction, ElicitationSchema, EnumSchema},
    service::ServiceError,
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{LazyLock, Mutex},
    time::Duration,
};

pub(super) struct PendingApproval<'a> {
    guard: &'a Safeguard,
    request_id: &'a str,
    key: (PathBuf, String),
    cleanup: bool,
}

static ACTIVE: LazyLock<Mutex<HashSet<(PathBuf, String)>>> = LazyLock::new(Mutex::default);

impl<'a> PendingApproval<'a> {
    pub(super) fn claim(
        mcp: &'a Mcp,
        request_id: &'a str,
        fingerprint: &str,
    ) -> Result<Option<Self>> {
        let key = (mcp.runtime.state.root().to_owned(), fingerprint.to_owned());
        let mut active = ACTIVE
            .lock()
            .map_err(|_| anyhow::anyhow!("Approval request lock is unavailable"))?;
        Ok(active.insert(key.clone()).then(|| Self {
            guard: &mcp.runtime.guard,
            request_id,
            key,
            cleanup: false,
        }))
    }

    pub(super) fn arm(&mut self) {
        self.cleanup = true;
    }

    pub(super) fn release(mut self) {
        self.cleanup = false;
    }
}

impl Drop for PendingApproval<'_> {
    fn drop(&mut self) {
        if self.cleanup {
            self.guard
                .cancel_pending_request(self.request_id, Some(&self.key.1));
        }
        if let Ok(mut active) = ACTIVE.lock() {
            active.remove(&self.key);
        }
    }
}

#[async_trait::async_trait]
impl DialogElicitor for TrustedRequestContext {
    fn verified_owner(&self, _account: &str) -> Option<i64> {
        self.dialog_owner()
    }

    fn has_form_capability(&self) -> bool {
        self.supports_form_elicitation
    }

    fn protocol(&self) -> &str {
        self.protocol_version.as_deref().unwrap_or_default()
    }

    async fn elicit(&self, prompt: &ApprovalPrompt<'_>) -> ChannelOutcome {
        use ChannelOutcome::*;
        if !dialog_available(
            self.dialog_owner().is_some(),
            self.has_form_capability(),
            self.protocol(),
        ) {
            return Unsupported;
        }
        let Some(peer) = self.peer() else {
            return Unsupported;
        };
        let mut choices = vec!["once".to_owned(), "deny".to_owned()];
        if prompt.allow_always {
            choices.push("always".to_owned());
        }
        let Ok(schema) = ElicitationSchema::builder()
            .title("Telegram MCP approval")
            .required_enum_schema("decision", EnumSchema::builder(choices).build())
            .build()
        else {
            return Unsupported;
        };
        let timeout = prompt
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        if timeout.is_zero() {
            return TimedOut;
        }
        let request = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: prompt.text.to_owned(),
            requested_schema: schema,
        };
        match peer
            .create_elicitation_with_timeout(request, Some(timeout))
            .await
        {
            Ok(result) => match result.action {
                ElicitationAction::Cancel => Cancelled,
                ElicitationAction::Accept => {
                    match result.content.as_ref().and_then(|v| v["decision"].as_str()) {
                        Some("once") => ApprovedOnce,
                        Some("always") if prompt.allow_always => ApprovedAlways,
                        _ => Denied,
                    }
                }
                _ => Denied,
            },
            Err(ServiceError::Cancelled { .. }) => Cancelled,
            Err(ServiceError::Timeout { .. }) => TimedOut,
            Err(_) => AmbiguousDelivery,
        }
    }
}

pub(super) struct SavedMessages<'a> {
    account: &'a str,
    api: &'a dyn Telegram,
    owner: Option<i64>,
}

pub(super) fn positive_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|id| *id > 0)
}

impl<'a> SavedMessages<'a> {
    pub(super) async fn observe(
        guard: &Safeguard,
        account: &'a str,
        api: &'a dyn Telegram,
    ) -> Self {
        let owner = if api.supports_saved_messages_approval() {
            match tokio::time::timeout(
                Duration::from_secs(3),
                api.invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]})),
            )
            .await
            {
                Ok(Ok(Value::Array(users))) => match users.as_slice() {
                    [user] if user["_"] == "user" && user["bot"] != true => {
                        positive_id(&user["id"])
                    }
                    _ => None,
                },
                _ => None,
            }
        } else {
            None
        };
        let observed = Self {
            account,
            api,
            owner,
        };
        guard.observe_approval_account(account, observed.availability());
        observed
    }

    pub(super) fn availability(&self) -> ChannelAvailability {
        if self.owner.is_some() {
            ChannelAvailability::Available
        } else {
            ChannelAvailability::Unavailable
        }
    }

    pub(super) fn routes<'b>(
        dialog: &'b TrustedRequestContext,
        saved: Option<&'b Self>,
    ) -> ApprovalRoutes<'b> {
        ApprovalRoutes {
            dialog: Some(dialog),
            saved_messages: saved.map(|adapter| adapter as &dyn SavedMessagesTransport),
        }
    }
}

fn sent_message_id(response: &Value, random_id: &Value) -> Option<i32> {
    match response["_"].as_str() {
        Some("updateShortSentMessage") => positive_id(&response["id"]),
        Some("updates" | "updatesCombined") => {
            let mut ids = response["updates"]
                .as_array()?
                .iter()
                .filter(|item| item["_"] == "updateMessageID" && item["random_id"] == *random_id);
            let first = positive_id(&ids.next()?["id"])?;
            ids.next().is_none().then_some(first)
        }
        _ => None,
    }
    .and_then(|id| i32::try_from(id).ok())
}

fn matches_user(peer: &Value, owner: i64) -> bool {
    peer["_"] == "peerUser" && positive_id(&peer["user_id"]) == Some(owner)
}

fn saved_reply(message: &Value, owner: i64, prompt_id: i32) -> Option<SavedReply> {
    let reply_to = &message["reply_to"];
    let from = &message["from_id"];
    let peer = &reply_to["reply_to_peer_id"];
    if message["_"] != "message"
        || positive_id(&message["id"])? <= i64::from(prompt_id)
        || !matches_user(&message["peer_id"], owner)
        || !(matches_user(from, owner) || (from.is_null() && message["out"] == true))
        || !message["fwd_from"].is_null()
        || !message["via_bot_id"].is_null()
        || !message["via_business_bot_id"].is_null()
        || reply_to["_"] != "messageReplyHeader"
        || !(peer.is_null() || matches_user(peer, owner))
        || positive_id(&reply_to["reply_to_msg_id"]) != Some(i64::from(prompt_id))
    {
        return None;
    }
    Some(SavedReply {
        owner,
        self_peer: owner,
        reply_to_message_id: prompt_id,
        text: message["message"].as_str()?.to_owned(),
    })
}

#[async_trait::async_trait]
impl SavedMessagesTransport for SavedMessages<'_> {
    fn verified_self_peer(&self, account: &str) -> Option<i64> {
        self.owner
            .filter(|_| self.account.eq_ignore_ascii_case(account))
    }

    async fn send_prompt(
        &self,
        prompt: &ApprovalPrompt<'_>,
        owner: i64,
    ) -> std::result::Result<i32, ChannelOutcome> {
        if self.verified_self_peer(prompt.account) != Some(owner) {
            return Err(ChannelOutcome::Unsupported);
        }
        let nonce = crate::args::random_id();
        let reply = self
            .api
            .invoke(
                "messages.sendMessage",
                json!({
                    "peer":{"_":"inputPeerSelf"}, "message":prompt.text, "random_id":nonce,
                }),
            )
            .await
            .map_err(|_| ChannelOutcome::AmbiguousDelivery)?;
        sent_message_id(&reply, &nonce).ok_or(ChannelOutcome::AmbiguousDelivery)
    }

    async fn observe_reply(
        &self,
        prompt: &ApprovalPrompt<'_>,
        owner: i64,
        message_id: i32,
    ) -> std::result::Result<SavedReply, ChannelOutcome> {
        if self.verified_self_peer(prompt.account) != Some(owner) {
            return Err(ChannelOutcome::AmbiguousDelivery);
        }
        loop {
            if tokio::time::Instant::now() >= prompt.deadline {
                return Err(ChannelOutcome::TimedOut);
            }
            let result = self
                .api
                .invoke(
                    "messages.getHistory",
                    json!({
                        "peer":{"_":"inputPeerSelf"}, "offset_id":0, "offset_date":0,
                        "add_offset":0, "limit":50, "max_id":0, "min_id":message_id, "hash":"0",
                    }),
                )
                .await
                .map_err(|_| ChannelOutcome::AmbiguousDelivery)?;
            let messages = result["messages"]
                .as_array()
                .ok_or(ChannelOutcome::AmbiguousDelivery)?;
            let mut replies = messages
                .iter()
                .filter_map(|message| saved_reply(message, owner, message_id));
            if let Some(reply) = replies.next() {
                if replies.next().is_some() {
                    return Err(ChannelOutcome::AmbiguousDelivery);
                }
                return Ok(reply);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{Account, Runtime, access::AccessPolicy, catalogue};
    use crate::state::State;
    use std::{collections::HashMap, path::Path, sync::Arc};

    struct ActingClient {
        replies: Mutex<std::collections::VecDeque<&'static str>>,
        prompt: Mutex<(i32, String)>,
        prompted: tokio::sync::Notify,
        state: Arc<State>,
    }

    #[async_trait::async_trait]
    impl Telegram for ActingClient {
        fn supports_saved_messages_approval(&self) -> bool {
            true
        }
        async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
            Ok(match method {
                "users.getUsers" => json!([{"_":"user","id":"42"}]),
                "messages.sendMessage" => {
                    assert_eq!(params["peer"]["_"], "inputPeerSelf");
                    let mut prompt = self.prompt.lock().unwrap();
                    prompt.0 += 10;
                    prompt.1 = params["message"].as_str().unwrap().to_owned();
                    self.prompted.notify_one();
                    json!({"_":"updateShortSentMessage","id":prompt.0})
                }
                "messages.getHistory" if params["peer"]["_"] == "inputPeerSelf" => {
                    let Some(command) = self.replies.lock().unwrap().pop_front() else {
                        return Ok(json!({"messages":[]}));
                    };
                    let prompt = self.prompt.lock().unwrap();
                    let code = prompt
                        .1
                        .split_whitespace()
                        .last()
                        .unwrap()
                        .trim_end_matches('.');
                    assert_eq!(params["min_id"], prompt.0);
                    json!({"messages":[{"_":"message","id":prompt.0 + 1,"out":true,
                        "peer_id":{"_":"peerUser","user_id":"42"},
                        "reply_to":{"_":"messageReplyHeader","reply_to_msg_id":prompt.0},
                        "message":format!("{command} {code}")} ]})
                }
                "messages.getMessages" => json!({"messages":[{"_":"message","id":7,
                    "peer_id":{"_":"peerUser","user_id":"7"},"media":{"document":{"id":"11","access_hash":"22","file_reference":[1]}}}]}),
                "messages.getHistory" => {
                    let (id, body) = self.prompt.lock().unwrap().clone();
                    let code = body
                        .split_whitespace()
                        .last()
                        .unwrap()
                        .trim_end_matches('.');
                    let (first, rest) = code.split_at(6);
                    json!({"messages":[
                        {"_":"message","id":id,"peer_id":{"_":"peerUser","user_id":"42"},"message":"redact by message identity"},
                        {"_":"message","id":id+1,"peer_id":{"_":"peerUser","user_id":"7"},"message":format!("Quote: {body}")},
                        {"_":"message","id":id+2,"peer_id":{"_":"peerUser","user_id":"7"},"message":format!("Code: {first}\u{202e}{rest}")},
                        {"_":"message","id":id+3,"peer_id":{"_":"peerUser","user_id":"7"},"message":"Unrelated history"}
                    ]})
                }
                _ => anyhow::bail!("Unexpected fixture RPC {method}"),
            })
        }

        async fn resolve_peer(&self, selector: &Value) -> Result<Value> {
            Ok(json!({"_":"inputPeerUser","user_id":selector,"access_hash":"77"}))
        }

        async fn download(&self, _: Value, destination: &Path, _: u64) -> Result<Value> {
            crate::transfer::publish_bytes(&self.state, destination, b"fixture media")?;
            Ok(json!({"path":destination,"size":13}))
        }
    }

    fn fixture(replies: Vec<&'static str>) -> (tempfile::TempDir, Mcp, Arc<ActingClient>) {
        let temp = tempfile::tempdir().unwrap();
        let installation = temp.path().join("app");
        let files = installation.join("files");
        std::fs::create_dir_all(&files).unwrap();
        let state = Arc::new(
            State::at_with_installation(temp.path().join("private"), vec![files], installation)
                .unwrap(),
        );
        let api = Arc::new(ActingClient {
            replies: Mutex::new(replies.into()),
            prompt: Mutex::default(),
            prompted: tokio::sync::Notify::new(),
            state: Arc::clone(&state),
        });
        let slot = Arc::new(Account {
            api: api.clone(),
            serial: Arc::default(),
            generation: None,
        });
        let mut guard = Safeguard::new(Arc::clone(&state));
        guard.bot = None;
        let mcp = Mcp {
            runtime: Arc::new(Runtime {
                accounts: tokio::sync::Mutex::new(HashMap::from([("default".into(), slot)])),
                guard,
                state,
                tools: catalogue::catalogue_for_mode("all").unwrap(),
                policy: AccessPolicy::default(),
            }),
        };
        (temp, mcp, api)
    }

    #[tokio::test]
    async fn folder_denial_does_not_write_and_once_does_not_carry_to_retry() {
        let (temp, server, _) = fixture(vec!["DENY", "APPROVE", "DENY"]);
        let outside = temp.path().join("outside");
        let dest = outside.join("test.bin");
        std::fs::create_dir(&outside).unwrap();
        let args = json!({"account":"default","chat_id":7,"message_id":7,"file_path":dest});
        let error = server
            .execute("download_media", args.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Approval denied"));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        server
            .execute("download_media", args.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"fixture media");
        std::fs::remove_file(&dest).unwrap();
        let error = server.execute("download_media", args).await.unwrap_err();
        assert!(error.to_string().contains("Approval denied"));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(
            server
                .runtime
                .state
                .read_json("folder-grants.json", json!({"folders":[]}))
                .unwrap(),
            json!({"folders":[]})
        );
    }

    #[tokio::test]
    async fn terminal_once_authorizes_the_matching_folder_retry() {
        let (temp, server, _) = fixture(vec!["DENY"]);
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let target = outside.join("terminal.bin");
        let sibling = outside.join("sibling.bin");
        let args = json!({"account":"default","chat_id":7,"message_id":7,"file_path":target});
        let mut validated = args.clone();
        let tool = server
            .runtime
            .tools
            .iter()
            .find(|tool| tool.name == "download_media")
            .unwrap();
        crate::server::validate_arguments(&mut validated, &tool.input_schema).unwrap();
        let paths =
            super::super::dispatch::FolderPaths::for_call(&server, "download_media", &validated)
                .unwrap();
        let (_, mut bound) = super::super::recipient::bind(
            "download_media",
            server.account("default").await.unwrap().api.clone(),
            &validated,
        )
        .await
        .unwrap();
        paths.bind(&mut bound);
        let fingerprint = crate::safeguard::request_fingerprint("download_media", &bound).unwrap();
        server
            .runtime
            .state
            .write_json(
                "approval-grants.json",
                &json!({
                    fingerprint: {"expires":crate::safeguard::unix_seconds() + 60}
                }),
            )
            .unwrap();
        server
            .execute("download_media", args.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"fixture media");
        let mut other = args;
        other["file_path"] = json!(sibling);
        assert!(
            server
                .execute("download_media", other)
                .await
                .unwrap_err()
                .to_string()
                .contains("Approval denied")
        );
        assert!(!sibling.exists());
    }

    #[tokio::test]
    async fn cancellation_closes_ticket_and_projection_redacts_saved_messages() {
        let (temp, server, api) = fixture(vec![]);
        let dest = temp.path().join("cancelled.bin");
        let ctx = TrustedRequestContext::local();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let operation = server.execute_with_cancel(
            "download_media",
            json!({"account":"default","chat_id":7,"message_id":7,"file_path":dest}),
            &cancellation,
            &ctx,
        );
        let (outcome, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(operation, async {
                api.prompted.notified().await;
                cancellation.cancel();
            })
        })
        .await
        .unwrap();
        assert!(outcome.unwrap_err().to_string().contains("cancelled"));
        assert!(!dest.exists());
        let state = &server.runtime.state;
        assert_eq!(
            state.read_json("approval-pending.json", json!({})).unwrap(),
            json!({})
        );
        let result = server
            .execute("get_history", json!({"account":"default","chat_id":7}))
            .await
            .unwrap();
        let output = super::super::results::build_call_tool_result(None, result).content[0]
            .as_text()
            .unwrap()
            .text
            .clone();
        assert!(!output.contains("redact by message identity"));
        assert!(!output.contains("Reply with"));
        assert!(output.contains("[approval request - hidden]"));
        assert!(output.contains("Code: [hidden]"));
        assert!(output.contains("Unrelated history"));
    }
}
