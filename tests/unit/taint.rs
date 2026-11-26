use super::*;

#[test]
fn approval_redaction_hides_nested_cross_account_requotes_and_only_matching_message_ids()
-> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let state = std::sync::Arc::new(crate::state::State::at(
        directory.path().to_owned(),
        vec![],
    )?);
    state.write_json(
        "sealed.json",
        &json!({"codes":["A1BC"],"accounts":{"work":{"self_peer":7,"message_ids":[42]}}}),
    )?;
    let guard = crate::safeguard::Safeguard::new(state);
    let input =
        json!({"chat_id":"me","items":[{"id":42,"text":"private"},{"id":43,"text":"copy A1BC!"}]});
    assert_eq!(
        guard.redact_tool_result("work", &input),
        json!({"chat_id":"me","items":["[approval request - hidden]",{"id":43,"text":"copy [hidden]!"}]})
    );
    assert_eq!(
        guard.redact_tool_result("home", &input)["items"][0]["text"],
        "private"
    );
    assert_eq!(
        guard.redact_tool_result(
            "home",
            &json!("before Approval A1BC: details from another device. after")
        ),
        "before [approval request - hidden] after"
    );
    Ok(())
}

#[test]
fn repeated_short_rendered_text_keeps_window_storage_bounded_and_preserves_fragments() {
    let mut memory = Memory::default();
    for _ in 0..20_000 {
        memory.note_value("work", &json!(42), &json!({"message":"short @peer_canary"}));
    }
    let account = &memory.accounts["work"];
    assert_eq!(account.total, 0);
    assert!(
        account.batches.is_empty(),
        "Empty passage batches cannot consume unbounded memory"
    );
    assert!(
        !memory
            .find("work", &json!({"message":"@peer_canary"}))
            .is_empty()
    );
}

#[test]
fn copied_passages_after_the_old_prefix_limit_are_detected_on_both_sides() {
    let secret = "A confidential human conversation needs approval before sharing";
    let mut memory = Memory::default();
    memory.note_value(
        "work",
        &json!(42),
        &json!({"message":format!("{} {secret}", "x".repeat(5_000))}),
    );
    assert!(
        memory
            .find("work", &json!({"text":secret}))
            .iter()
            .any(|row| row["kind"] == "passage" && row["source_chat"] == 42)
    );
    let mut memory = Memory::default();
    memory.note_value("work", &json!(43), &json!({"message":secret}));
    assert!(
        memory
            .find(
                "work",
                &json!({"text":format!("{} {secret}", "y".repeat(5_000))})
            )
            .iter()
            .any(|row| row["kind"] == "passage" && row["source_chat"] == 43)
    );
}

#[test]
fn one_outgoing_text_reports_every_distinct_passage_source_once() {
    let first = "The project budget will be discussed by the finance committee";
    let second = "A personal medical appointment has been arranged for tomorrow";
    let mut memory = Memory::default();
    memory.note_value("work", &json!(42), &json!({"message":first}));
    memory.note_value("work", &json!(43), &json!({"message":second}));
    let found = memory.find(
        "work",
        &json!({"text":format!("{first}\n{second}\n{first}")}),
    );
    let mut chats = found
        .iter()
        .filter(|r| r["kind"] == "passage")
        .map(|r| r["source_chat"].as_i64().unwrap())
        .collect::<Vec<_>>();
    chats.sort();
    assert_eq!(chats, vec![42, 43]);
}

#[test]
fn full_text_scanning_keeps_the_recent_passage_retention_budget_bounded() {
    let mut text: String = (0x10000..0x10000 + MAX_WINDOWS as u32 + 10_000)
        .filter_map(char::from_u32)
        .collect();
    let tail = "The recent confidential passage must remain protected before sharing";
    text.push_str(tail);
    let mut memory = Memory::default();
    memory.note_value("work", &json!(42), &json!({"message":text}));
    let account = &memory.accounts["work"];
    assert!(account.total <= MAX_WINDOWS);
    assert!(account.windows.len() <= account.total);
    assert!(
        memory
            .find("work", &json!({"text":tail}))
            .iter()
            .any(|r| r["kind"] == "passage" && r["source_chat"] == 42)
    );
}
