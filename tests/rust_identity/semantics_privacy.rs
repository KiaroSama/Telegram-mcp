use super::common::*;
use serde_json::json;

#[tokio::test]
async fn privacy_read_preserves_rule_order_exceptions_and_unknown_future_rules() {
    let m = Mock::new();
    m.reply("account.getPrivacy",json!({"rules":[{"_":"privacyValueAllowUsers","users":[8]},{"_":"privacyValueDisallowChatParticipants","chats":[7]},{"_":"privacyValueFutureRule"}]}));
    let result = run(&m, "get_privacy_settings", json!({"key":"status"}))
        .await
        .unwrap();
    assert_eq!(
        result["results"],
        json!([{"rule":"users_allowed","users":[8]},{"rule":"chats_disallowed","chats":[7]},{"rule":"FutureRule"}])
    );
    assert_eq!(result["metadata"]["rule_count"], 3);
    assert!(m.sent("account.setPrivacy").is_empty());
}

#[tokio::test]
async fn complete_privacy_rule_sets_keep_chat_exceptions_before_the_fallback_policy() {
    let m = Mock::new();
    run(&m,"set_privacy_settings",json!({"key":"status","rules":[{"rule":"chats_allowed","chats":[7,"9"]},{"rule":"bots_disallowed"},{"rule":"everyone_disallowed"}]})).await.unwrap();
    let rules = &m.sent("account.setPrivacy")[0]["rules"];
    assert_eq!(rules[0]["chats"], json!(["7", "9"]));
    assert_eq!(rules[1]["_"], "inputPrivacyValueDisallowBots");
    assert_eq!(rules[2]["_"], "inputPrivacyValueDisallowAll");
}

#[tokio::test]
async fn all_rule_shapes_are_validated_before_any_user_selector_is_resolved() {
    for malformed in [
        json!({"rule":"unknown"}),
        json!({"rule":"chats_allowed","chats":"wrong"}),
        json!({"rule":"users_disallowed","users":7}),
    ] {
        let m = Mock::new();
        assert!(
            run(
                &m,
                "set_privacy_settings",
                json!({"rules":[{"rule":"users_allowed","users":[8]},malformed]})
            )
            .await
            .is_err()
        );
        assert!(m.resolutions().is_empty());
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn equivalent_numeric_and_string_selectors_cannot_enter_both_exception_sets() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_privacy_settings",
            json!({"base_policy":"contacts","allow_users":[8],"disallow_users":["8"]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.resolutions(), json!([8, "8"]).as_array().unwrap().clone());
    assert!(m.sent("account.setPrivacy").is_empty());
}

#[tokio::test]
async fn two_aliases_resolving_to_the_same_user_are_also_conflicting_exceptions() {
    let m = Mock::new();
    for alias in ["Ada", "@ada"] {
        m.set_peer(
            alias,
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"80"}),
        );
    }
    assert!(
        run(
            &m,
            "set_privacy_settings",
            json!({"base_policy":"nobody","allow_users":["Ada"],"disallow_users":["@ada"]})
        )
        .await
        .is_err()
    );
    assert!(m.sent("account.setPrivacy").is_empty());
}

#[tokio::test]
async fn distinct_user_exceptions_precede_each_normalized_base_policy() {
    for (policy, constructor) in [
        (" EVERYONE ", "inputPrivacyValueAllowAll"),
        ("Nobody", "inputPrivacyValueDisallowAll"),
    ] {
        let m = Mock::new();
        m.set_peer(
            "10",
            json!({"_":"inputPeerUser","user_id":"10","access_hash":"100"}),
        );
        run(
            &m,
            "set_privacy_settings",
            json!({"base_policy":policy,"allow_users":[8],"disallow_users":[10]}),
        )
        .await
        .unwrap();
        let rules = &m.sent("account.setPrivacy")[0]["rules"];
        assert_eq!(rules[0]["_"], "inputPrivacyValueAllowUsers");
        assert_eq!(rules[1]["_"], "inputPrivacyValueDisallowUsers");
        assert_eq!(rules[2]["_"], constructor);
        assert_eq!(rules[1]["users"][0]["user_id"], "10");
    }
}

#[tokio::test]
async fn non_user_selectors_cannot_be_written_as_user_privacy_exceptions() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_privacy_settings",
            json!({"base_policy":"contacts","allow_users":[7]})
        )
        .await
        .is_err()
    );
    assert!(m.sent("account.setPrivacy").is_empty());
}

#[tokio::test]
async fn invalid_privacy_key_policy_or_selector_containers_do_not_change_rules() {
    for args in [
        json!({"key":"not-a-key","base_policy":"everyone"}),
        json!({"base_policy":"friends"}),
        json!({"base_policy":"contacts","allow_users":true}),
        json!({"base_policy":"contacts","disallow_users":"8"}),
        json!({"rules":"not-an-array"}),
    ] {
        let m = Mock::new();
        assert!(run(&m, "set_privacy_settings", args).await.is_err());
        assert!(m.methods().is_empty());
        assert!(m.resolutions().is_empty());
    }
}
