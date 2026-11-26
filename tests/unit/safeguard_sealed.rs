use super::*;

fn fixture() -> Result<(tempfile::TempDir, Arc<State>)> {
    let root = tempfile::tempdir()?;
    let state = Arc::new(State::at(root.path().to_owned(), vec![])?);
    Ok((root, state))
}

fn saved_request() -> ReplyBinding<'static> {
    ReplyBinding {
        request_id: "request",
        owner: 7,
        channel: ApprovalChannel::SavedMessagesChannel,
        account: "work",
        self_peer: Some(7),
        message_id: None,
        code: "A1BC",
    }
}

#[test]
fn legacy_import_preserves_account_scope_and_redacts_requoted_content() -> Result<()> {
    let (_root, state) = fixture()?;
    state.write_json(FILE, &json!({"codes":["a1bc","2345"],"accounts":{"Work":{"selves":["7","Owner"],"messages":[42]}}}))?;
    let seals = SealedStore::new(state);
    assert!(seals.is_sealed_message("work", " @OWNER ", 42));
    assert!(seals.is_sealed_target("WORK", &json!({"chat_id":"me","message_ids":["42"]})));
    assert!(!seals.is_sealed_message("home", "me", 42));
    assert!(!seals.is_sealed_message("work", "8", 42));
    assert!(seals.is_sealed_target("home", &json!({"text":"yes a1bc"})));
    assert_eq!(
        seals
            .redact_text("before Approval A1BC: request\nfrom another device. after A1BC! A1BCdef"),
        "before [approval request - hidden] after [hidden]! A1BCdef"
    );
    assert_eq!(
        seals.redact_value(&json!({"a1bc":[2345,"A1bc!","genuine text"]})),
        json!({"[hidden]":["[hidden]","[hidden]!","genuine text"]})
    );
    Ok(())
}

#[test]
fn only_the_original_live_request_can_consume_a_delivered_reply() -> Result<()> {
    let (_root, state) = fixture()?;
    let seals = SealedStore::new(Arc::clone(&state));
    let start = saved_request();
    let reply = ReplyBinding {
        message_id: Some(23),
        ..start
    };
    let decision = ChannelOutcome::ApprovedOnce;
    seals.begin(PendingRequest::new(start, 40)?, &["private\nrequest"])?;
    assert!(seals.is_sealed_message("work", "me", 99));
    assert!(!seals.consume(reply, decision, 15)?);
    seals.bind_message(reply, 15)?;
    assert!(!seals.is_sealed_message("work", "me", 99));
    assert!(!seals.consume(ReplyBinding { owner: 8, ..reply }, decision, 15)?);
    assert!(!SealedStore::new(Arc::clone(&state)).consume(reply, decision, 15)?);
    assert!(seals.consume(reply, decision, 15)?);
    assert!(!seals.consume(reply, decision, 15)?);
    let reloaded = SealedStore::new(state);
    assert!(reloaded.is_sealed_message("work", "7", 23));
    assert_eq!(
        reloaded.redact_text("x private\nrequest y private\\nrequest z"),
        "x [approval request - hidden] y [approval request - hidden] z"
    );
    Ok(())
}

#[test]
fn ambiguous_delivery_keeps_self_messages_sealed_after_restart() -> Result<()> {
    let (_root, state) = fixture()?;
    let seals = SealedStore::new(Arc::clone(&state));
    let start = saved_request();
    seals.begin(
        PendingRequest::new(start, 40)?,
        &["Private approval prompt"],
    )?;
    seals.close(start.request_id, ChannelOutcome::AmbiguousDelivery)?;
    let reloaded = SealedStore::new(state);
    assert!(reloaded.is_sealed_target("work", &json!({"chat_id":"7"})));
    assert!(!reloaded.is_sealed_target("home", &json!({"chat_id":"7"})));
    Ok(())
}

#[test]
fn corruption_and_failed_delivery_binding_are_conservative() -> Result<()> {
    let (_root, state) = fixture()?;
    let seals = SealedStore::new(Arc::clone(&state));
    let start = saved_request();
    seals.begin(
        PendingRequest::new(start, 40)?,
        &["Private approval prompt"],
    )?;
    std::fs::create_dir(state.root().join("account-rename-transaction.json"))?;
    let reply = ReplyBinding {
        message_id: Some(23),
        ..start
    };
    assert!(seals.bind_message(reply, 10).is_err());
    assert!(
        seals
            .consume(reply, ChannelOutcome::ApprovedOnce, 10)
            .is_err()
    );
    assert_eq!(seals.redact_text("unverified"), HIDDEN_REQUEST);
    let reloaded = SealedStore::new(Arc::clone(&state));
    assert!(reloaded.is_sealed_message("work", "me", 42));
    std::fs::write(state.root().join(FILE), b"{}")?;
    assert!(reloaded.is_sealed_target("unknown", &json!({})));
    assert_eq!(
        reloaded.redact_value(&json!({"content":["potential prompt"]})),
        json!(HIDDEN_REQUEST)
    );
    assert!(reloaded.codes().is_err());
    Ok(())
}
