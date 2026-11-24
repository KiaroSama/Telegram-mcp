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
#[derive(Default)]
struct SecretPeers(std::sync::Mutex<Vec<Value>>);

#[async_trait]
impl Telegram for SecretPeers {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        bail!("Recipient and policy preflight should not dispatch a tool")
    }

    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        self.0.lock().unwrap().push(value.clone());
        let id = value.as_i64().unwrap_or(7);
        Ok(json!({"_":"inputPeerUser","user_id":id,"access_hash":13}))
    }
}

#[tokio::test]
async fn secret_recipient_binding_resolves_only_cloud_fields() {
    use crate::server::recipient::bind;
    use std::sync::Arc;

    let api = Arc::new(SecretPeers::default());
    for (name, args, expected) in [
        (
            "create_secret_chat",
            json!({"user_id":"@allowed"}),
            json!({"user_id":"7"}),
        ),
        (
            "copy_into_secret_chat",
            json!({"from_chat_id":7,"to_chat_id":7}),
            json!({"from_chat_id":"7","to_chat_id":"secret/7"}),
        ),
        (
            "export_secret_chat",
            json!({"secret_chat_id":7,"peer_user":"@allowed"}),
            json!({"secret_chat_id":"secret/7","peer_user":"7"}),
        ),
        (
            "export_secret_chat",
            json!({"secret_chat_id":7,"peer_user":null}),
            json!({"secret_chat_id":"secret/7","peer_user":null}),
        ),
        (
            "forward_secret_media",
            json!({"from_chat_id":7,"to_chat_id":8}),
            json!({"from_chat_id":"secret/7","to_chat_id":"secret/8"}),
        ),
        (
            "clear_secret_history",
            json!({"chat_id":-1_999_999_999_993i64,"confirm_chat_id":-1_999_999_999_993i64}),
            json!({"chat_id":"secret/7","confirm_chat_id":"secret/7"}),
        ),
    ] {
        let (_, mut approval) = bind(name, api.clone(), &args).await.unwrap();
        approval
            .as_object_mut()
            .unwrap()
            .remove("_requested_peer_targets");
        assert_eq!(approval, expected, "{name}");
    }
    assert_eq!(
        *api.0.lock().unwrap(),
        vec![json!("@allowed"), json!(7), json!("@allowed")]
    );
}

#[tokio::test]
async fn secret_allowlist_requires_an_explicit_wire_id_not_a_matching_cloud_peer() {
    let api = SecretPeers::default();
    let cloud = AccessPolicy::parse(Some("7,-1999999999993")).unwrap();
    for args in [
        json!({"secret_chat_id":7}),
        json!({"secret_chat_id":-1_999_999_999_993i64}),
    ] {
        assert!(cloud.check("close_secret_chat", &args, &api).await.is_err());
    }
    let secret = AccessPolicy::parse(Some("secret/7")).unwrap();
    for id in [7i64, -1_999_999_999_993, 2_000_000_000_007] {
        secret
            .check("close_secret_chat", &json!({"secret_chat_id":id}), &api)
            .await
            .unwrap();
    }
    assert!(api.0.lock().unwrap().is_empty());
    assert!(
        secret
            .check("get_chat", &json!({"chat_id":7}), &api)
            .await
            .is_err()
    );
    assert!(AccessPolicy::parse(Some("secret/7:3")).is_err());
}

#[tokio::test]
async fn secret_allowlist_blocks_unscoped_calls_and_requires_every_secret_target() {
    let api = SecretPeers::default();
    let policy = AccessPolicy::parse(Some("secret/7")).unwrap();
    for (name, args) in [
        ("list_secret_chats", json!({})),
        ("secret_chat_status", json!({"chat_id":null})),
        ("get_secret_backend_status", json!({})),
        ("start_secret_auto_save", json!({"folder":"exports"})),
        (
            "forward_secret_media",
            json!({"from_chat_id":8,"to_chat_id":7}),
        ),
        (
            "forward_secret_media",
            json!({"from_chat_id":7,"to_chat_id":8}),
        ),
        (
            "clear_secret_history",
            json!({"chat_id":7,"confirm_chat_id":8}),
        ),
    ] {
        assert!(policy.check(name, &args, &api).await.is_err(), "{name}");
    }
    policy
        .check(
            "forward_secret_media",
            &json!({"from_chat_id":7,"to_chat_id":7}),
            &api,
        )
        .await
        .unwrap();
    AccessPolicy::default()
        .check("list_secret_chats", &json!({}), &api)
        .await
        .unwrap();
    assert!(api.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn secret_search_results_are_filtered_to_the_requested_secret_chat() {
    let policy = AccessPolicy::parse(Some("secret/7,secret/8,8"))
        .unwrap()
        .scoped(
            "search_secret_messages",
            &json!({"chat_id":7,"query":"test"}),
            &SecretPeers::default(),
        )
        .await
        .unwrap();
    let mut value = json!({"messages":[{"chat_id":7},{"chat_id":8}]});
    policy
        .filter_for("search_secret_messages", &mut value)
        .unwrap();
    assert_eq!(value["messages"], json!([{"chat_id":7}]));
}

#[tokio::test]
async fn secret_allowlist_checks_cloud_source_and_optional_export_user_separately() {
    let api = SecretPeers::default();
    let policy = AccessPolicy::parse(Some("secret/7,8")).unwrap();
    for (name, args, allowed) in [
        ("create_secret_chat", json!({"user_id":8}), true),
        ("create_secret_chat", json!({"user_id":7}), false),
        (
            "copy_into_secret_chat",
            json!({"from_chat_id":8,"to_chat_id":7}),
            true,
        ),
        (
            "copy_into_secret_chat",
            json!({"from_chat_id":7,"to_chat_id":7}),
            false,
        ),
        (
            "export_secret_chat",
            json!({"secret_chat_id":7,"peer_user":8}),
            true,
        ),
        (
            "export_secret_chat",
            json!({"secret_chat_id":7,"peer_user":7}),
            false,
        ),
        ("export_secret_chat", json!({"secret_chat_id":7}), true),
    ] {
        assert_eq!(
            policy.check(name, &args, &api).await.is_ok(),
            allowed,
            "{name}"
        );
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
