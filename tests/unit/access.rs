use super::*;
use async_trait::async_trait;

struct Peers(i64);

#[tokio::test]
async fn account_wide_readers_and_linked_community_mutations_cannot_escape_chat_scope() {
    let policy = AccessPolicy::parse(Some("allowed")).unwrap();
    let tools = crate::server::catalogue().unwrap();
    for (name, mut args) in [
        ("get_contact_ids", json!({})),
        ("get_blocked_users", json!({})),
        (
            "import_contacts",
            json!({"contacts":[{"phone":(["+44","7700900123"].concat()),"first_name":"Fixture","last_name":"Contact"}]}),
        ),
        ("list_folders", json!({})),
        ("get_folder", json!({"folder_id":3})),
        (
            "ban_community_member",
            json!({"community":"allowed","user":"target","also_from_chats":true}),
        ),
    ] {
        let schema = &tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .input_schema;
        crate::server::validate_arguments(&mut args, schema).unwrap();
        assert!(
            policy.scoped(name, &args, &Peers(7)).await.is_err(),
            "{name} must reject its unscoped account-wide reach before dispatch"
        );
        assert!(
            AccessPolicy::default()
                .scoped(name, &args, &Peers(7))
                .await
                .is_ok()
        );
    }
    policy
        .scoped(
            "ban_community_member",
            &json!({"community":"allowed","user":"target","also_from_chats":false}),
            &Peers(7),
        )
        .await
        .unwrap();
}
#[async_trait]
impl Telegram for Peers {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        bail!("Policy checks do not invoke arbitrary Telegram methods")
    }
    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        match value.as_str() {
            Some("@allowed" | "allowed" | "@alias" | "alias") => Ok(
                json!({"_":"inputPeerChannel","channel_id":self.0.to_string(),"access_hash":"13"}),
            ),
            _ => Ok(json!({"_":"inputPeerChannel","channel_id":"9","access_hash":"19"})),
        }
    }
}
fn message(id: i64) -> Value {
    json!({"_":"message","id":1,"peer_id":{"_":"peerChannel","channel_id":id.to_string()},"message":"body"})
}

#[tokio::test]
async fn username_scoped_history_preserves_only_its_proven_numeric_peer_without_chat_metadata() {
    let original = AccessPolicy::parse(Some("@allowed")).unwrap();
    let scoped = original
        .scoped("get_messages", &json!({"chat_id":"@allowed"}), &Peers(7))
        .await
        .unwrap();
    let mut result = json!({"messages":[message(7),message(9)]});
    scoped.filter_for("get_messages", &mut result).unwrap();
    assert_eq!(result["messages"], json!([message(7)]));
    assert!(
        !original.chats.contains_key("-1000000000007"),
        "The shared policy must not accumulate request/account bindings"
    );
}

#[tokio::test]
async fn username_bindings_are_separate_for_each_resolved_account() {
    let original = AccessPolicy::parse(Some("allowed")).unwrap();
    let left = original
        .scoped("get_history", &json!({"chat_id":"@allowed"}), &Peers(7))
        .await
        .unwrap();
    let right = original
        .scoped("get_history", &json!({"chat_id":"@allowed"}), &Peers(8))
        .await
        .unwrap();
    let mut left_result = json!({"messages":[message(7),message(8)]});
    let mut right_result = left_result.clone();
    left.filter(&mut left_result).unwrap();
    right.filter(&mut right_result).unwrap();
    assert_eq!(left_result["messages"], json!([message(7)]));
    assert_eq!(right_result["messages"], json!([message(8)]));
}

#[tokio::test]
async fn response_usernames_never_grant_access_to_an_unresolved_peer() {
    let policy = AccessPolicy::parse(Some("allowed"))
        .unwrap()
        .scoped("get_history", &json!({"chat_id":"@allowed"}), &Peers(7))
        .await
        .unwrap();
    let mut outside = message(9);
    outside["username"] = json!("allowed");
    let mut result = json!({"messages":[message(7),outside],"chats":[{"_":"channel","id":"7","username":"allowed"},{"_":"channel","id":"9","username":"allowed"}],"nested":{"messages":[message(9)]}});
    policy.filter(&mut result).unwrap();
    assert_eq!(result["messages"], json!([message(7)]));
    assert_eq!(result["chats"].as_array().unwrap().len(), 1);
    assert_eq!(result["chats"][0]["id"], "7");
    assert_eq!(result["nested"]["messages"], json!([]));
}

#[tokio::test]
async fn listing_resolves_allowed_usernames_before_filtering_public_numeric_rows() {
    let policy = AccessPolicy::parse(Some("allowed"))
        .unwrap()
        .scoped("list_chats", &json!({}), &Peers(7))
        .await
        .unwrap();
    let mut result = json!({"results":[{"id":"-1000000000007","username":"allowed"},{"id":"-1000000000009","username":"allowed"}]});
    policy.filter_for("list_chats", &mut result).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["id"], "-1000000000007");
}

#[tokio::test]
async fn resolved_topic_scope_stays_narrow_and_whole_chat_permission_has_priority() {
    let narrow = AccessPolicy::parse(Some("allowed:5,-1000000000007:6")).unwrap();
    assert!(
        narrow
            .scoped("get_history", &json!({"chat_id":"@allowed"}), &Peers(7))
            .await
            .is_err()
    );
    assert!(
        narrow
            .scoped(
                "delete_message",
                &json!({"chat_id":"@allowed","topic_id":5}),
                &Peers(7)
            )
            .await
            .is_err()
    );
    let scoped = narrow
        .scoped(
            "get_history",
            &json!({"chat_id":"@allowed","topic_id":5}),
            &Peers(7),
        )
        .await
        .unwrap();
    assert_eq!(scoped.chats["-1000000000007"], Some(BTreeSet::from([5, 6])));
    assert!(!scoped.allows_event("-1000000000007"));
    let broad = AccessPolicy::parse(Some("allowed:5,-1000000000007"))
        .unwrap()
        .scoped("get_history", &json!({"chat_id":"@allowed"}), &Peers(7))
        .await
        .unwrap();
    assert!(broad.allows_event("-1000000000007"));
}

#[tokio::test]
async fn denied_argument_cannot_inherit_a_binding_from_another_authorized_target() {
    let policy = AccessPolicy::parse(Some("allowed")).unwrap();
    assert!(
        policy
            .scoped(
                "forward_message",
                &json!({"from_chat_id":"@allowed","to_chat_id":"@other"}),
                &Peers(7)
            )
            .await
            .is_err()
    );
    assert!(!policy.chats.contains_key("-1000000000007"));
}

#[test]
fn exported_contacts_are_filtered_by_the_same_private_identity_policy() {
    let policy = AccessPolicy::parse(Some("42")).unwrap();
    let mut value = json!({"results":[{"id":"42","name":"Permitted"},{"id":"99","name":"Outside"}],"untrusted_content":true});
    policy.filter_for("export_contacts", &mut value).unwrap();
    assert_eq!(value["results"], json!([{"id":"42","name":"Permitted"}]));
}
#[tokio::test]
async fn public_lookup_links_are_normalized_before_allowlist_resolution() {
    let policy = AccessPolicy::parse(Some("-1000000000007")).unwrap();
    policy
        .scoped(
            "lookup_peer",
            &json!({"query":"https://t.me/allowed/123?single"}),
            &Peers(7),
        )
        .await
        .unwrap();
    for query in ["https://t.me/+private", "https://t.me/joinchat/private"] {
        assert!(
            policy
                .scoped("lookup_peer", &json!({"query":query}), &Peers(7))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn topic_restricted_replies_bind_the_referenced_message_before_approval() {
    struct Replies {
        topic: i64,
        reads: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl Telegram for Replies {
        async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
            assert_eq!(
                method, "channels.getMessages",
                "Topic validation cannot send a mutation"
            );
            crate::tl::Schema::global().encode_call(method, &args)?;
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(
                json!({"messages":[{"_":"message","id":10,"peer_id":{"_":"peerChannel","channel_id":"7"},"reply_to":{"forum_topic":true,"reply_to_top_id":self.topic},"message":"Untrusted target text"}]}),
            )
        }
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            Ok(json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}))
        }
    }
    let policy = AccessPolicy::parse(Some("allowed:5")).unwrap();
    for (name, mut args) in [
        (
            "reply_to_message",
            json!({"chat_id":"allowed","message_id":10,"topic_id":5,"text":"Safe scoped reply"}),
        ),
        (
            "send_message",
            json!({"chat_id":"allowed","reply_to_message_id":10,"topic_id":5,"message":"Safe scoped reply"}),
        ),
    ] {
        let tools = crate::server::catalogue().unwrap();
        let schema = &tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .input_schema;
        crate::server::validate_arguments(&mut args, schema).unwrap();
        for topic in [9, 5] {
            let api = Replies {
                topic,
                reads: std::sync::atomic::AtomicUsize::new(0),
            };
            let scoped = policy.scoped(name, &args, &api).await;
            assert_eq!(
                scoped.is_ok(),
                topic == 5,
                "The valid {name} reply schema must enforce its source topic"
            );
            assert_eq!(api.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }
}
