//! Latest-upstream event, privacy and argument boundaries through the server.
use super::*;

fn event(account: &str, chat: i64, topic: i64, username: &str, sender: &str) -> Value {
    json!({"account":account,"chat_id":chat,"chat_type":"supergroup","message_id":7,"sender_id":42,"sender_name":sender,"out":false,"topic":topic,"_chat_username":username})
}

#[tokio::test]
async fn username_topic_event_permission_survives_public_metadata_projection() {
    let policy = access::AccessPolicy::parse(Some("@Scoped_Forum:7")).unwrap();
    let (mcp, api, _tmp) = service(json!({}), policy.clone());
    for value in [
        event("other", -1000000000007, 7, "Scoped_Forum", "Allowed"),
        event("other", -1000000000007, 8, "Scoped_Forum", "Wrong topic"),
        event("other", -1000000000008, 7, "Other_Forum", "Wrong chat"),
    ] {
        mcp.runtime.state.record_message_event(value).unwrap();
    }
    let result = mcp
        .execute("wait_for_events", json!({"timeout":0}))
        .await
        .unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    assert_eq!(result["events"][0]["sender_name"], "Allowed");
    assert_eq!(result["events"][0]["account"], "other");
    assert_eq!(result["events"][0]["topic"], 7);
    assert!(result["events"][0].get("_chat_username").is_none());
    assert_eq!(result["access_filtered"], true);
    assert!(api.calls.lock().unwrap().is_empty());
}

#[test]
fn event_permission_requires_matching_topic_for_numeric_or_username_scope() {
    let policy = access::AccessPolicy::parse(Some("-1000000000007:7,@Scoped_Forum:9")).unwrap();
    assert!(policy.allows_message_event("-1000000000007", None, Some(7)));
    assert!(policy.allows_message_event("-1000000000008", Some("scoped_forum"), Some(9)));
    assert!(!policy.allows_message_event("-1000000000007", None, None));
    assert!(!policy.allows_message_event("-1000000000007", Some("scoped_forum"), Some(8)));
    assert!(!policy.allows_message_event("-1000000000008", Some("other"), Some(9)));
    let whole = access::AccessPolicy::parse(Some("@Scoped_Forum")).unwrap();
    assert!(whole.allows_message_event("-1000000000008", Some("SCOPED_FORUM"), None));
}

#[tokio::test]
async fn global_event_wait_taints_only_actual_event_account_and_chat() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    mcp.runtime
        .state
        .record_message_event(event(
            "other",
            -1000000000007,
            7,
            "Scoped_Forum",
            "Contact @event_sender_instruction",
        ))
        .unwrap();
    let events = mcp
        .execute("wait_for_events", json!({"timeout":0}))
        .await
        .unwrap();
    assert_eq!(events["events"][0]["account"], "other");
    let allowed = mcp
        .execute(
            "create_business_chat_link",
            json!({"account":"default","text":"Contact @event_sender_instruction"}),
        )
        .await
        .unwrap();
    assert!(allowed.get("approval_required").is_none());
    let guarded = mcp
        .execute(
            "create_business_chat_link",
            json!({"account":"other","text":"Contact @event_sender_instruction"}),
        )
        .await
        .unwrap();
    assert_eq!(guarded["approval_required"], true);
    assert!(
        guarded["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "tainted")
    );
    assert_eq!(api.calls.lock().unwrap().len(), 1);
    let pending = mcp
        .runtime
        .state
        .read_json("approval-pending.json", json!({}))
        .unwrap();
    let record = &pending[guarded["fingerprint"].as_str().unwrap()];
    assert_eq!(record["arguments"]["account"], "other");
}

#[tokio::test]
async fn explicit_account_event_wait_ignores_the_account_operation_serial_lock() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    mcp.runtime
        .state
        .record_message_event(event(
            "default",
            -1000000000007,
            7,
            "Scoped_Forum",
            "Allowed",
        ))
        .unwrap();
    let slot = mcp
        .runtime
        .accounts
        .lock()
        .await
        .get("default")
        .unwrap()
        .clone();
    let _held = slot.serial.lock().await;
    let result = tokio::time::timeout(
        Duration::from_millis(200),
        mcp.execute("wait_for_events", json!({"account":"default","timeout":0})),
    )
    .await
    .expect("Event waits must not wait for a domain operation's account lock")
    .unwrap();
    assert_eq!(result["events"][0]["account"], "default");
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restricted_folder_snapshot_and_approved_patch_never_reach_rpc() {
    let policy = access::AccessPolicy::parse(Some("7")).unwrap();
    let (mcp, api, _tmp) = service(json!({}), policy);
    assert!(
        mcp.execute("get_folder_snapshot", json!({"account":"default"}))
            .await
            .is_err()
    );
    let args = json!({"account":"default","folder_id":7,"patch":{"title":"Changed"}});
    assert!(mcp.execute("update_folder", args.clone()).await.is_err());
    // Even a correctly normalized, exact terminal grant cannot widen chat
    // access. Policy now refuses before consuming or requesting approval.
    let mut normalized = args.clone();
    let descriptor = mcp
        .runtime
        .tools
        .iter()
        .find(|tool| tool.name == "update_folder")
        .unwrap();
    validate_arguments(&mut normalized, &descriptor.input_schema).unwrap();
    let fingerprint = crate::safeguard::request_fingerprint("update_folder", &normalized).unwrap();
    mcp.runtime
        .state
        .update_json("approval-grants.json", json!({}), |grants| {
            grants[&fingerprint] = json!({"expires":crate::safeguard::unix_seconds()+60});
            Ok(())
        })
        .unwrap();
    assert!(mcp.execute("update_folder", args).await.is_err());
    assert!(api.calls.lock().unwrap().is_empty());
    assert!(
        mcp.runtime
            .state
            .read_json("approval-grants.json", json!({}))
            .unwrap()
            .get(&fingerprint)
            .is_some()
    );
}

#[tokio::test]
async fn latest_catalogue_enum_validation_precedes_approval_and_network() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    for (name, args) in [
        (
            "transcribe_voice",
            json!({"account":"default","chat_id":7,"message_id":20,"engine":"unconfigured-provider"}),
        ),
        (
            "edit_admin_rights",
            json!({"account":"default","chat_id":7,"user_id":8,"mode":"merge"}),
        ),
    ] {
        assert!(mcp.execute(name, args).await.is_err());
    }
    let tool = mcp
        .runtime
        .tools
        .iter()
        .find(|tool| tool.name == "transcribe_voice")
        .unwrap();
    for engine in [
        Value::Null,
        json!("telegram"),
        json!("groq"),
        json!("openai"),
        json!("whisper"),
    ] {
        let mut args = json!({"chat_id":7,"message_id":20,"engine":engine});
        validate_arguments(&mut args, &tool.input_schema).unwrap();
    }
    assert!(api.calls.lock().unwrap().is_empty());
    assert!(
        !mcp.runtime
            .state
            .root()
            .join("approval-pending.json")
            .exists()
    );
}

#[tokio::test]
async fn wait_with_empty_owned_event_log_does_not_open_an_unconfigured_account() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    mcp.runtime.accounts.lock().await.clear();
    let result = mcp
        .execute("wait_for_events", json!({"timeout":0}))
        .await
        .unwrap();
    assert_eq!(result["events"], json!([]));
    assert!(api.calls.lock().unwrap().is_empty());
    let probe = Probe {
        calls: StdMutex::new(vec![]),
        reply: json!({}),
    };
    let owned = crate::tools::local::upstream_events::wait_for_events_with_state(
        &probe,
        &mcp.runtime.state,
        &json!({"timeout":0,"after_seq":-1}),
        &mcp.runtime.policy,
    )
    .await
    .unwrap();
    assert_eq!(owned["events"], json!([]));
    assert_eq!(owned["last_seq"], 0);
    assert!(probe.calls.lock().unwrap().is_empty());
}
