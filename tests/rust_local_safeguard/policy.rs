use super::*;

#[test]
fn safeguard_category_tables_do_not_overlap() {
    let sets = [
        tables::GATED,
        tables::SEND,
        tables::SEEN_SIGNAL,
        tables::FREE_WRITES,
    ];
    for (i, left) in sets.iter().enumerate() {
        for right in sets.iter().skip(i + 1) {
            assert!(!left.iter().any(|name| right.contains(name)));
        }
    }
}
#[test]
fn unknown_destructive_tool_is_gated() {
    assert_eq!(categorize("new_wipe", false, true), Category::Gated);
}
#[test]
fn reads_are_free_by_default() {
    assert_eq!(categorize("get_history", true, false), Category::Free);
}
#[test]
fn exports_still_require_approval_despite_readonly_hint() {
    assert_eq!(
        categorize("export_chat_history", true, false),
        Category::Gated
    );
}
#[test]
fn ordinary_writes_preserve_owner_policy() {
    for name in [
        "send_message",
        "reply_to_message",
        "edit_message",
        "send_reaction",
    ] {
        assert_eq!(
            decide(name, false, true, &json!({}), &Facts::default()).outcome,
            "run"
        );
    }
}
#[test]
fn deletion_and_session_mutations_ask() {
    for name in [
        "delete_message",
        "ban_user",
        "leave_chat",
        "terminate_authorization",
        "update_profile",
        "set_profile_photo",
    ] {
        assert_eq!(
            decide(name, false, true, &json!({}), &Facts::default()).outcome,
            "ask"
        );
    }
}
#[test]
fn first_send_to_stranger_asks() {
    let facts = Facts {
        first_message: true,
        ..Default::default()
    };
    assert_eq!(
        decide("send_message", false, true, &json!({}), &facts).reasons,
        vec!["first_message"]
    );
}
#[test]
fn first_message_fact_does_not_gate_edit() {
    let facts = Facts {
        first_message: true,
        ..Default::default()
    };
    assert_eq!(
        decide("edit_message", false, true, &json!({}), &facts).outcome,
        "run"
    );
}
#[test]
fn bulk_send_asks() {
    let facts = Facts {
        bulk: true,
        ..Default::default()
    };
    assert_eq!(
        decide("send_message", false, true, &json!({}), &facts).reasons,
        vec!["bulk_send"]
    );
}
#[test]
fn tainted_write_carries_source_metadata() {
    let facts = Facts {
        tainted: vec![json!({"kind":"url","source_chat":"42"})],
        ..Default::default()
    };
    let decision = decide("send_message", false, true, &json!({}), &facts);
    assert_eq!(decision.reasons, vec!["tainted"]);
    assert_eq!(decision.tainted, facts.tainted);
}
#[test]
fn tainted_read_stays_readonly() {
    let facts = Facts {
        tainted: vec![json!({"kind":"url"})],
        ..Default::default()
    };
    assert_eq!(
        decide("resolve_username", true, false, &json!({}), &facts).outcome,
        "run"
    );
}
#[test]
fn seen_signals_under_ghost_require_approval() {
    let facts = Facts {
        ghost_on: true,
        ..Default::default()
    };
    assert_eq!(
        decide("mark_as_read", false, true, &json!({}), &facts).reasons,
        vec!["seen_signal_under_ghost"]
    );
    assert_eq!(
        decide("mark_as_read", false, true, &json!({}), &Facts::default()).outcome,
        "run"
    );
}
#[test]
fn turning_ghost_off_asks_and_on_is_free() {
    assert_eq!(
        decide(
            "set_ghost_mode",
            false,
            false,
            &json!({"enabled":false}),
            &Facts::default()
        )
        .reasons,
        vec!["ghost_off"]
    );
    assert_eq!(
        decide(
            "set_ghost_mode",
            false,
            false,
            &json!({"enabled":true}),
            &Facts::default()
        )
        .outcome,
        "run"
    );
}
