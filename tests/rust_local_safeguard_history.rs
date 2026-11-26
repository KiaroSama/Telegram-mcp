mod local_support;
use local_support::fixture;
use serde_json::json;
use telegram_mcp::safeguard::{Decision, Safeguard};

#[test]
fn historical_approval_messages_and_requoted_codes_stay_sealed_with_bypass() {
    let (_dir, state) = fixture();
    state
        .write_json(
            "sealed.json",
            &json!({"codes":["A1BC"],"accounts":{"work":{"selves":["7","Owner"],"messages":[42]}}}),
        )
        .unwrap();
    state
        .write_json("bypass.json", &json!({"on":true,"until":null,"by":7}))
        .unwrap();
    let guard = Safeguard::new(state.clone());
    for args in [
        json!({"account":"Work","chat_id":"@Owner","message_id":42}),
        json!({"account":"home","chat_id":8,"quote":"copy A1BC!"}),
    ] {
        assert!(matches!(
            guard.check("delete_message", &args, false, true).unwrap(),
            Decision::Refuse { .. }
        ));
    }
    assert_eq!(
        guard
            .check(
                "delete_message",
                &json!({"account":"home","chat_id":"me","message_id":42}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
    std::fs::write(state.root().join("sealed.json"), b"{").unwrap();
    assert!(matches!(
        guard
            .check(
                "delete_message",
                &json!({"account":"work","chat_id":30,"message_id":2}),
                false,
                true
            )
            .unwrap(),
        Decision::Refuse { .. }
    ));
}

#[test]
fn successful_send_history_survives_guard_restart_and_stays_account_scoped() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state.clone());
    let args = json!({"account":"work","chat_id":42,"text":"Synthetic message"});
    assert!(matches!(
        guard.check("send_message", &args, false, true).unwrap(),
        Decision::NeedsApproval { .. }
    ));
    guard.note_success("send_message", &args).unwrap();
    let restarted = Safeguard::new(state.clone());
    assert_eq!(
        restarted.check("send_message", &args, false, true).unwrap(),
        Decision::Allow
    );
    assert!(matches!(
        restarted
            .check(
                "send_message",
                &json!({"account":"personal","chat_id":42,"text":"Synthetic message"}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
    for chat in 43..=47 {
        guard
            .note_success("send_message", &json!({"account":"work","chat_id":chat}))
            .unwrap();
    }
    let decision = guard
        .check(
            "send_message",
            &json!({"account":"work","chat_id":42,"text":"Synthetic message"}),
            false,
            true,
        )
        .unwrap();
    assert!(
        matches!(decision,Decision::NeedsApproval{reasons,..} if reasons.iter().any(|reason|reason=="bulk_send"))
    );
    assert!(
        guard
            .note_success("send_message", &json!({"account":"work"}))
            .is_err()
    );
}

#[test]
fn send_history_retention_is_bounded_and_corruption_never_becomes_a_trusted_recipient() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state.clone());
    guard
        .note_success("edit_message", &json!({"account":"work","chat_id":42}))
        .unwrap();
    assert!(!state.root().join("sent-recipients.json").exists());
    state
        .write_json(
            "sent-recipients.json",
            &json!({"work":(1..=10000).map(|id|id.to_string()).collect::<Vec<_>>()}),
        )
        .unwrap();
    guard
        .note_success("send_message", &json!({"account":"work","chat_id":10001}))
        .unwrap();
    let known = state.read_json("sent-recipients.json", json!({})).unwrap();
    let rows = known["work"].as_array().unwrap();
    assert_eq!(rows.len(), 10000);
    assert!(!rows.contains(&json!("1")));
    assert!(rows.contains(&json!("10001")));
    state
        .write_json("sent-recipients.json", &json!({"work":"corrupt"}))
        .unwrap();
    let before = std::fs::read(state.root().join("sent-recipients.json")).unwrap();
    assert!(
        guard
            .note_success("send_message", &json!({"account":"work","chat_id":42}))
            .is_err()
    );
    assert_eq!(
        std::fs::read(state.root().join("sent-recipients.json")).unwrap(),
        before
    );
}

#[test]
fn untrusted_link_retention_evicts_old_fragments_and_refreshes_repeated_links() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    for index in 0..5001 {
        guard.note_rendered(
            "work",
            &json!(42),
            &json!({"text":format!("https://x{index}.t")}),
        );
    }
    assert_eq!(
        guard
            .check(
                "edit_message",
                &json!({"account":"work","chat_id":42,"text":"https://x0.t"}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
    assert!(matches!(
        guard
            .check(
                "edit_message",
                &json!({"account":"work","chat_id":42,"text":"https://x5000.t"}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
    guard.note_rendered("work", &json!(99), &json!({"text":"https://x1.t"}));
    guard.note_rendered("work", &json!(99), &json!({"text":"https://new.t"}));
    assert!(matches!(
        guard
            .check(
                "edit_message",
                &json!({"account":"work","chat_id":42,"text":"https://x1.t"}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
    assert_eq!(
        guard
            .check(
                "edit_message",
                &json!({"account":"personal","chat_id":42,"text":"https://x1.t"}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
}
