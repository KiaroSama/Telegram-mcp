use super::*;

struct DialogFixture {
    protocol: &'static str,
    answer: ChannelOutcome,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl DialogElicitor for DialogFixture {
    fn verified_owner(&self, account: &str) -> Option<i64> {
        (account == "work").then_some(7)
    }
    fn has_form_capability(&self) -> bool {
        true
    }
    fn protocol(&self) -> &str {
        self.protocol
    }
    async fn elicit(&self, _: &ApprovalPrompt<'_>) -> ChannelOutcome {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.answer
    }
}

struct SavedFixture<'a> {
    seals: &'a super::super::sealed::SealedStore,
    sent: std::sync::Mutex<String>,
}

#[async_trait::async_trait]
impl SavedMessagesTransport for SavedFixture<'_> {
    fn verified_self_peer(&self, account: &str) -> Option<i64> {
        (account == "work").then_some(7)
    }
    async fn send_prompt(
        &self,
        prompt: &ApprovalPrompt<'_>,
        owner: i64,
    ) -> std::result::Result<i32, ChannelOutcome> {
        assert_eq!(owner, 7);
        let code = prompt
            .text
            .split_whitespace()
            .last()
            .unwrap()
            .trim_end_matches('.');
        assert!(self.seals.codes().unwrap().contains(code));
        *self.sent.lock().unwrap() = code.to_owned();
        Ok(61)
    }
    async fn observe_reply(
        &self,
        _: &ApprovalPrompt<'_>,
        owner: i64,
        id: i32,
    ) -> std::result::Result<SavedReply, ChannelOutcome> {
        assert!(self.seals.is_sealed_message("work", "me", id));
        let code = self.sent.lock().unwrap().clone();
        Ok(SavedReply {
            owner,
            self_peer: owner,
            reply_to_message_id: id,
            text: format!("APPROVE {code}"),
        })
    }
}

#[tokio::test]
async fn real_router_uses_only_supported_routes_and_seals_saved_messages() -> Result<()> {
    use super::super::{Decision, Safeguard};
    use ChannelOutcome::{
        AmbiguousDelivery, ApprovedOnce, Cancelled, Denied, TimedOut, Unsupported,
    };
    use serde_json::json;
    use std::sync::{Arc, atomic::Ordering};
    for (protocol, answer, expected) in [
        ("2026-07-28", Denied, ApprovedOnce),
        ("2025-11-25", Unsupported, ApprovedOnce),
        ("2025-11-25", Denied, Denied),
        ("2025-11-25", Cancelled, Cancelled),
        ("2025-11-25", TimedOut, TimedOut),
        ("2025-11-25", AmbiguousDelivery, AmbiguousDelivery),
    ] {
        // Use a fresh seal store per route so each fixture can use the same Telegram ID.
        let directory = tempfile::tempdir()?;
        let state = Arc::new(crate::state::State::at(
            directory.path().to_owned(),
            vec![],
        )?);
        let mut guard = Safeguard::new(state);
        guard.bot = None;
        let dialog = DialogFixture {
            protocol,
            answer,
            calls: 0.into(),
        };
        let saved = SavedFixture {
            seals: &guard.seals,
            sent: Default::default(),
        };
        let args = json!({"account":"work","chat_id":42,"message_id":1});
        let Decision::NeedsApproval {
            fingerprint,
            reasons,
        } = guard.check("delete_message", &args, false, true)?
        else {
            panic!("A fixture mutation needs approval");
        };
        let routes = ApprovalRoutes {
            dialog: Some(&dialog),
            saved_messages: Some(&saved),
        };
        assert_eq!(
            guard
                .request_owner_approval(
                    &routes,
                    &fingerprint,
                    "delete_message",
                    &args,
                    &reasons,
                    None
                )
                .await?,
            expected
        );
        assert_eq!(
            dialog.calls.load(Ordering::Relaxed),
            usize::from(protocol != "2026-07-28")
        );
        assert_eq!(
            !saved.sent.lock().unwrap().is_empty(),
            expected == ChannelOutcome::ApprovedOnce
        );
        if expected == ChannelOutcome::ApprovedOnce {
            assert_eq!(
                guard.check("delete_message", &args, false, true)?,
                Decision::Allow
            );
        }
        assert!(matches!(
            guard.check("delete_message", &args, false, true)?,
            Decision::NeedsApproval { .. }
        ));
    }
    Ok(())
}

#[test]
fn only_unsupported_permits_another_channel() {
    assert!(ChannelOutcome::Unsupported.may_fall_through());
    for outcome in [
        ChannelOutcome::Denied,
        ChannelOutcome::Cancelled,
        ChannelOutcome::TimedOut,
        ChannelOutcome::AmbiguousDelivery,
        ChannelOutcome::ApprovedOnce,
        ChannelOutcome::ApprovedAlways,
    ] {
        assert!(!outcome.may_fall_through(), "{outcome:?}");
    }
}

#[test]
fn dialog_requires_trusted_form_capability_and_a_compatible_version() {
    assert!(dialog_available(true, true, "2025-11-25"));
    assert!(dialog_available(true, true, "2026-07-27"));
    assert!(!dialog_available(false, true, "2025-11-25"));
    assert!(!dialog_available(true, false, "2025-11-25"));
    for protocol in [
        "2026-07-28",
        "2026-07-29",
        "2028-01-01",
        "garbage",
        "2026-02-30",
        "2026-7-9",
    ] {
        assert!(!dialog_available(true, true, protocol), "{protocol}");
    }
}

#[test]
fn pending_requires_the_delivered_message_and_exact_live_context() -> Result<()> {
    let start = ReplyBinding {
        request_id: "request_7",
        owner: 7,
        channel: ApprovalChannel::SavedMessagesChannel,
        account: "Work",
        self_peer: Some(7),
        message_id: None,
        code: "ABCD",
    };
    let mut pending = PendingRequest::new(start, 20)?;
    let reply = ReplyBinding {
        message_id: Some(23),
        ..start
    };
    assert!(!pending.matches_reply(reply, 10));
    assert!(
        pending
            .bind_message(
                ReplyBinding {
                    self_peer: Some(8),
                    ..reply
                },
                10
            )
            .is_err()
    );
    pending.bind_message(reply, 10)?;
    assert!(pending.matches_reply(
        ReplyBinding {
            code: "abcd",
            ..reply
        },
        19
    ));
    for wrong in [
        ReplyBinding {
            request_id: "other",
            ..reply
        },
        ReplyBinding { owner: 8, ..reply },
        ReplyBinding {
            channel: ApprovalChannel::BotChannel,
            ..reply
        },
        ReplyBinding {
            account: "home",
            ..reply
        },
        ReplyBinding {
            self_peer: Some(8),
            ..reply
        },
        ReplyBinding {
            message_id: Some(24),
            ..reply
        },
        ReplyBinding {
            code: "EFGH",
            ..reply
        },
        start,
    ] {
        assert!(!pending.matches_reply(wrong, 19));
    }
    assert!(!pending.matches_reply(reply, 20));
    pending.close();
    assert!(!pending.matches_reply(reply, 19));
    Ok(())
}
