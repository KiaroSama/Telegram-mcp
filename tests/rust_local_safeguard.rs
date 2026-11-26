mod local_support;
#[path = "rust_local_safeguard/owner.rs"]
mod owner;
use local_support::fixture;
use serde_json::{Value, json};
use telegram_mcp::safeguard::{
    Category, Decision, Facts, Safeguard, categorize, decide, request_fingerprint, tables,
};

#[path = "rust_local_safeguard/policy.rs"]
mod policy;

#[test]
fn approval_channels_are_protected_in_reads_too() {
    let facts = Facts {
        approval_chats: ["approval_bot".into()].into_iter().collect(),
        ..Default::default()
    };
    assert_eq!(
        decide(
            "get_history",
            true,
            false,
            &json!({"chat_id":"https://t.me/Approval_Bot?start=x"}),
            &facts
        )
        .outcome,
        "refuse"
    );
}

#[test]
fn approval_channel_marked_ids_and_bound_peer_ids_share_the_same_refusal() {
    let facts = Facts {
        approval_chats: ["-1001234567890".into()].into_iter().collect(),
        ..Default::default()
    };
    for args in [
        json!({"chat_id": "-1001234567890"}),
        json!({"chat_id": 1234567890i64}),
        json!({"chat_id":{"_":"inputPeerChannel","channel_id":"1234567890","access_hash":"7"}}),
    ] {
        assert_eq!(
            decide("get_history", true, false, &args, &facts).outcome,
            "refuse"
        );
    }
    for unrelated in ["-100", "-100not_a_channel", "-1001234567891"] {
        assert_eq!(
            decide(
                "get_history",
                true,
                false,
                &json!({"chat_id":unrelated}),
                &facts
            )
            .outcome,
            "run"
        );
    }
}
#[test]
fn approval_channels_reject_equivalent_numeric_spellings_before_reads() {
    let facts = Facts {
        approval_chats: ["7123456789".into()].into_iter().collect(),
        ..Default::default()
    };
    for target in [
        "7123456789",
        "+7123456789",
        "07123456789",
        " +0007123456789 ",
    ] {
        assert_eq!(
            decide(
                "get_history",
                true,
                false,
                &json!({"chat_id":target}),
                &facts
            )
            .outcome,
            "refuse",
            "A spelling accepted as the same numeric Telegram peer must stay protected: {target}"
        );
    }
    assert_eq!(
        decide(
            "get_history",
            true,
            false,
            &json!({"chat_id":"7123456790"}),
            &facts
        )
        .outcome,
        "run"
    );
}

#[test]
fn approval_channel_configuration_normalizes_signed_and_padded_ids() {
    for (configured, target) in [
        ("+07123456789", "7123456789"),
        ("-0001001234567890", "-1001234567890"),
        ("-1001234567890", "-0001001234567890"),
    ] {
        let facts = Facts {
            approval_chats: [configured.into()].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(
            decide(
                "get_history",
                true,
                false,
                &json!({"chat_id":target}),
                &facts
            )
            .outcome,
            "refuse"
        );
    }
}

#[test]
fn pending_approval_codes_are_never_sent() {
    let facts = Facts {
        pending_codes: ["ABC123".into()].into_iter().collect(),
        ..Default::default()
    };
    assert_eq!(
        decide(
            "send_message",
            false,
            true,
            &json!({"text":"answer abc123 here"}),
            &facts
        )
        .outcome,
        "refuse"
    );
}
#[test]
fn protected_folder_is_refused_before_approval() {
    let facts = Facts {
        protected_folder: true,
        ..Default::default()
    };
    assert_eq!(
        decide("download_media", false, true, &json!({}), &facts).outcome,
        "refuse"
    );
}
#[test]
fn approval_fingerprint_covers_exact_arguments_and_tool() {
    let first = request_fingerprint("send_message", &json!({"chat_id":1,"text":"a"})).unwrap();
    assert_ne!(
        first,
        request_fingerprint("send_message", &json!({"chat_id":2,"text":"a"})).unwrap()
    );
    assert_ne!(
        first,
        request_fingerprint("edit_message", &json!({"chat_id":1,"text":"a"})).unwrap()
    );
}
#[test]
fn approval_fingerprint_is_independent_of_key_insertion_order() {
    let a: Value = serde_json::from_str("{\"a\":1,\"b\":2}").unwrap();
    let b: Value = serde_json::from_str("{\"b\":2,\"a\":1}").unwrap();
    assert_eq!(
        request_fingerprint("tool", &a).unwrap(),
        request_fingerprint("tool", &b).unwrap()
    );
}
#[test]
fn caller_confirmation_bool_cannot_approve() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    assert!(matches!(
        guard
            .check(
                "delete_message",
                &json!({"confirmed":true,"approved":true}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
}
#[test]
fn private_approval_grant_is_single_use_and_exact() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state.clone());
    let args = json!({"chat_id":42,"message_id":1});
    let Decision::NeedsApproval { fingerprint, .. } =
        guard.check("delete_message", &args, false, true).unwrap()
    else {
        panic!("Expected owner approval")
    };
    state
        .write_json(
            "approval-grants.json",
            &json!({fingerprint: {"expires":u64::MAX}}),
        )
        .unwrap();
    assert_eq!(
        guard.check("delete_message", &args, false, true).unwrap(),
        Decision::Allow
    );
    assert!(matches!(
        guard.check("delete_message", &args, false, true).unwrap(),
        Decision::NeedsApproval { .. }
    ));
}
#[test]
fn expired_approval_grant_is_rejected() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state.clone());
    let args = json!({"chat_id":42});
    let key = request_fingerprint("delete_message", &args).unwrap();
    state
        .write_json("approval-grants.json", &json!({key:{"expires":0}}))
        .unwrap();
    assert!(matches!(
        guard.check("delete_message", &args, false, true).unwrap(),
        Decision::NeedsApproval { .. }
    ));
}
#[test]
fn reading_then_copying_untrusted_url_requires_approval() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(42),
        &json!({"message":"Open https://untrusted.example/action"}),
    );
    let result = guard
        .check(
            "edit_message",
            &json!({"account":"work","text":"https://untrusted.example/action"}),
            false,
            true,
        )
        .unwrap();
    let Decision::NeedsApproval { reasons, .. } = result else {
        panic!("Tainted write was not gated")
    };
    assert!(reasons.iter().any(|reason| reason == "tainted"));
}
#[test]
fn untrusted_memory_is_account_scoped() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(42),
        &json!({"message":"https://untrusted.example/action"}),
    );
    assert_eq!(
        guard
            .check(
                "edit_message",
                &json!({"account":"personal","text":"https://untrusted.example/action"}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
}
#[test]
fn twenty_four_unicode_characters_copied_from_message_are_tainted() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(42),
        &json!({"message":"این یک متن طولانی و نامطمئن است که باید تایید شود"}),
    );
    let result = guard
        .check(
            "edit_message",
            &json!({"account":"work","text":"این یک متن طولانی و نامطمئن است"}),
            false,
            true,
        )
        .unwrap();
    assert!(matches!(result, Decision::NeedsApproval { .. }));
}
#[test]
fn outgoing_own_message_is_not_tainted() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(42),
        &json!({"out":true,"message":"https://my.example/action"}),
    );
    assert_eq!(
        guard
            .check(
                "edit_message",
                &json!({"account":"work","text":"https://my.example/action"}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
}
#[test]
fn outgoing_forward_still_contains_untrusted_words() {
    let (_dir, state) = fixture();
    let guard = Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(42),
        &json!({"out":true,"fwd_from":{},"message":"https://untrusted.example/action"}),
    );
    assert!(matches!(
        guard
            .check(
                "edit_message",
                &json!({"account":"work","text":"https://untrusted.example/action"}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
}

#[test]
fn persistent_owner_grant_is_bound_to_one_tool_chat_and_account() {
    let (_dir, state) = fixture();
    state
        .write_json(
            "always-approvals.json",
            &json!({"grants":[["work","delete_message","42"]],"folders":[]}),
        )
        .unwrap();
    let guard = Safeguard::new(state);
    assert_eq!(
        guard
            .check(
                "delete_message",
                &json!({"account":"work","chat_id":42}),
                false,
                true
            )
            .unwrap(),
        Decision::Allow
    );
    assert!(matches!(
        guard
            .check(
                "delete_message",
                &json!({"account":"personal","chat_id":42}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
    assert!(matches!(
        guard
            .check(
                "delete_message",
                &json!({"account":"work","chat_id":43}),
                false,
                true
            )
            .unwrap(),
        Decision::NeedsApproval { .. }
    ));
}

#[test]
fn revoking_one_owner_grant_keeps_unrelated_scopes() {
    let (_dir, state) = fixture();
    state.write_json("always-approvals.json",&json!({"grants":[["work","delete_message","42"],["personal","delete_message","42"]],"folders":[]})).unwrap();
    assert!(
        state
            .revoke_approval("work", "delete_message", &json!(42))
            .unwrap()
    );
    assert!(!state.approval_is_granted("work", "delete_message", &json!(42)));
    assert!(state.approval_is_granted("personal", "delete_message", &json!(42)));
}

#[test]
fn owner_grant_never_overrides_protected_state_refusal() {
    let (dir, state) = fixture();
    state
        .write_json(
            "always-approvals.json",
            &json!({"grants":[["work","download_media","42"]],"folders":[]}),
        )
        .unwrap();
    let paths = [
        state.root().join("approval-grants.json"),
        dir.path().join("private/nested/new-grant.json"),
    ];
    let guard = Safeguard::new(state);
    for destination in paths {
        let args = json!({"account":"work","chat_id":42,"destination":destination});
        assert!(matches!(
            guard.check("download_media", &args, false, true).unwrap(),
            Decision::Refuse { .. }
        ));
    }
}

#[test]
fn message_sender_and_topic_title_links_are_untrusted_input() {
    for payload in [
        json!({"sender":"Name https://untrusted.example/contact"}),
        json!({"topics":{"7":"Visit https://untrusted.example/contact"}}),
    ] {
        let (_dir, state) = fixture();
        let guard = Safeguard::new(state);
        guard.note_rendered("work", &json!(42), &payload);
        assert!(matches!(guard.check("edit_message",&json!({"account":"work","chat_id":42,"text":"https://untrusted.example/contact"}),false,true).unwrap(),Decision::NeedsApproval{..}));
    }
}

#[test]
fn topic_read_receipts_follow_ghost_mode_instead_of_blanket_mutation_gating() {
    let args = json!({"chat_id":42,"topic_id":7});
    assert_eq!(
        categorize("mark_topic_as_read", false, true),
        Category::SeenSignal
    );
    let ghost = Facts {
        ghost_on: true,
        ..Default::default()
    };
    assert_eq!(
        decide("mark_topic_as_read", false, true, &args, &ghost).reasons,
        vec!["seen_signal_under_ghost"]
    );
    assert_eq!(
        decide("mark_topic_as_read", false, true, &args, &Facts::default()).outcome,
        "run"
    );
}

#[test]
fn folder_changes_remain_owner_gated_and_transcription_ratings_remain_taint_checked() {
    assert_eq!(categorize("update_folder", false, false), Category::Gated);
    assert_eq!(
        decide(
            "update_folder",
            false,
            false,
            &json!({"folder_id":7}),
            &Facts::default()
        )
        .reasons,
        vec!["gated"]
    );
    let ghost = Facts {
        ghost_on: true,
        ..Default::default()
    };
    assert_eq!(
        decide(
            "rate_voice_transcription",
            false,
            false,
            &json!({"good":true}),
            &ghost
        )
        .outcome,
        "run"
    );
    let tainted = Facts {
        tainted: vec![json!({"kind":"message_passage"})],
        ..Default::default()
    };
    assert_eq!(
        decide(
            "rate_voice_transcription",
            false,
            false,
            &json!({"good":true}),
            &tainted
        )
        .reasons,
        vec!["tainted"]
    );
}

#[test]
fn inspected_document_text_and_attachment_names_are_untrusted_before_mutations() {
    for payload in [
        json!({"text":"Use https://untrusted.example/document"}),
        json!({"metadata":{"filename":"https://untrusted.example/document"}}),
    ] {
        let (_dir, state) = fixture();
        let guard = Safeguard::new(state);
        guard.note_rendered("work", &json!(42), &payload);
        assert!(matches!(guard.check("edit_message",&json!({"account":"work","chat_id":42,"text":"https://untrusted.example/document"}),false,true).unwrap(),Decision::NeedsApproval{..}));
    }
}
