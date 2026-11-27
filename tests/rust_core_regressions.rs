//! Independent regression probes found while reviewing the native core.
use anyhow::{Result, bail};
use async_trait::async_trait;
use base64::Engine;
use grammers_session::{
    Session,
    types::{PeerId, PeerInfo, UpdateState},
};
use serde_json::{Value, json};
use std::sync::Arc;
use telegram_mcp::{
    backend::Telegram, server::access::AccessPolicy, session::PersistentSession, state::State,
};

struct Peers;
#[async_trait]
impl Telegram for Peers {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        bail!("No network call expected")
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        let n = input
            .as_i64()
            .or_else(|| input.as_str()?.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Expected test peer id"))?;
        Ok(match n {
            7 => json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"}),
            9 => json!({"_":"inputPeerChat","chat_id":"9"}),
            _ => json!({"_":"inputPeerUser","user_id":n.to_string(),"access_hash":"80"}),
        })
    }
}
macro_rules! denied_target {
    ($test:ident,$tool:literal,$args:expr) => {
        #[tokio::test]
        async fn $test() {
            let p = AccessPolicy::parse(Some("-1000000000007")).unwrap();
            assert!(
                p.check($tool, &$args, &Peers).await.is_err(),
                "The alternate peer field bypassed the chat allowlist"
            );
        }
    };
}
denied_target!(
    chat_named_delete_target_is_restricted,
    "delete_chat",
    json!({"chat":9,"both_sides":false})
);
denied_target!(
    chat_named_pin_target_is_restricted,
    "pin_chat",
    json!({"chat":9})
);
denied_target!(
    channel_named_subscription_target_is_restricted,
    "subscribe_public_channel",
    json!({"channel":9})
);
denied_target!(
    contact_named_private_read_target_is_restricted,
    "get_contact_chats",
    json!({"contact_id":8})
);
denied_target!(
    community_secondary_link_target_is_restricted,
    "add_chat_to_community",
    json!({"community":7,"chat":9,"visibility":"hidden"})
);
denied_target!(
    numeric_lookup_query_is_restricted,
    "lookup_peer",
    json!({"query":8})
);
denied_target!(
    full_profile_username_target_is_restricted,
    "get_full_user",
    json!({"username":8})
);

#[tokio::test]
async fn allowed_group_member_is_not_mistaken_for_another_target_chat() {
    let p = AccessPolicy::parse(Some("-1000000000007")).unwrap();
    p.check("ban_user", &json!({"chat_id":7,"user_id":8}), &Peers)
        .await
        .unwrap();
}
#[tokio::test]
async fn allowed_group_invitation_does_not_require_members_to_be_allowed_private_chats() {
    let p = AccessPolicy::parse(Some("-1000000000007")).unwrap();
    p.check(
        "invite_to_group",
        &json!({"group_id":7,"user_ids":[8,10]}),
        &Peers,
    )
    .await
    .unwrap();
}
#[test]
fn allowed_marked_channels_and_their_dialog_rows_survive_output_filtering() {
    let p = AccessPolicy::parse(Some("-1000000000007")).unwrap();
    let mut result = json!({"chats":[{"_":"channel","id":"7","title":"Allowed"},{"_":"channel","id":"9","title":"Outside"}],"dialogs":[{"_":"dialog","peer":{"_":"peerChannel","channel_id":"7"}},{"_":"dialog","peer":{"_":"peerChannel","channel_id":"9"}}],"messages":[{"_":"message","id":1,"peer_id":{"_":"peerChannel","channel_id":"7"},"message":"Allowed"},{"_":"message","id":2,"peer_id":{"_":"peerChannel","channel_id":"9"},"message":"Outside"}]});
    p.filter(&mut result).unwrap();
    assert_eq!(result["chats"].as_array().unwrap().len(), 1);
    assert_eq!(result["chats"][0]["id"], "7");
    assert_eq!(result["dialogs"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
}
#[test]
fn standalone_contact_result_container_is_filtered_by_private_chat_id() {
    let p = AccessPolicy::parse(Some("8")).unwrap();
    let mut result = json!({"results":[{"id":"8","name":"Allowed"},{"id":"10","name":"Outside"}]});
    p.filter_for("list_contacts", &mut result).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["id"], "8");
}
#[test]
fn peer_namespaces_do_not_widen_channel_allowance_to_user_with_same_bare_id() {
    let p = AccessPolicy::parse(Some("-1000000000007")).unwrap();
    let mut result = json!({"results":[{"id":"7","name":"Different user"}]});
    p.filter_for("list_contacts", &mut result).unwrap();
    assert_eq!(result["results"], json!([]));
}

fn private_state() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let installation = temp.path().join("installation");
    std::fs::create_dir(&installation).unwrap();
    let state = Arc::new(
        State::at_with_installation(temp.path().join("state"), vec![files], installation).unwrap(),
    );
    (temp, state)
}
fn prevent_snapshot_replacement(state: &State, label: &str) {
    let path = state.root().join(format!("{label}.session.json"));
    if path.is_file() {
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::create_dir(path).unwrap();
}
#[tokio::test]
async fn failed_home_datacenter_write_preserves_in_memory_revision() {
    let (_temp, state) = private_state();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    session.set_home_dc_id(2).await.unwrap();
    prevent_snapshot_replacement(&state, "work");
    assert!(session.set_home_dc_id(3).await.is_err());
    assert_eq!(session.home_dc_id().unwrap(), 2);
}
#[tokio::test]
async fn failed_update_checkpoint_does_not_advance_memory_past_durable_state() {
    let (_temp, state) = private_state();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    session
        .set_update_state(UpdateState::Primary {
            pts: 10,
            date: 20,
            seq: 30,
        })
        .await
        .unwrap();
    prevent_snapshot_replacement(&state, "work");
    assert!(
        session
            .set_update_state(UpdateState::Primary {
                pts: 11,
                date: 21,
                seq: 31
            })
            .await
            .is_err()
    );
    let current = session.updates_state().await.unwrap();
    assert_eq!((current.pts, current.date, current.seq), (10, 20, 30));
}
#[tokio::test]
async fn failed_peer_cache_write_does_not_install_undurable_access_hash() {
    let (_temp, state) = private_state();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    prevent_snapshot_replacement(&state, "work");
    assert!(
        session
            .cache_peer(PeerInfo::User {
                id: 8,
                auth: None,
                bot: Some(false),
                is_self: Some(false)
            })
            .await
            .is_err()
    );
    let peer = PeerId::from_bot_api_dialog_id(8).unwrap();
    assert!(session.peer(peer).await.unwrap().is_none());
}
#[tokio::test]
async fn failed_datacenter_key_write_does_not_install_undurable_authorization() {
    let (_temp, state) = private_state();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    let mut dc = session.dc_option(2).unwrap().unwrap();
    assert!(dc.auth_key.is_none());
    dc.auth_key = Some([0x5a; 256]);
    prevent_snapshot_replacement(&state, "work");
    assert!(session.set_dc_option(&dc).await.is_err());
    assert!(session.dc_option(2).unwrap().unwrap().auth_key.is_none());
}

fn dummy_session_string() -> String {
    let mut bytes = vec![2, 149, 154, 167, 51];
    bytes.extend(443_u16.to_be_bytes());
    bytes.extend([0x5a; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}
#[test]
fn identical_authorization_cannot_be_active_under_two_labels_and_lease_releases_on_drop() {
    let (_temp, state) = private_state();
    let first = PersistentSession::open(Arc::clone(&state), "first").unwrap();
    let second = PersistentSession::open(Arc::clone(&state), "second").unwrap();
    first
        .import_string_session(&dummy_session_string())
        .unwrap();
    assert!(
        second
            .import_string_session(&dummy_session_string())
            .is_err()
    );
    assert!(second.dc_option(2).unwrap().unwrap().auth_key.is_none());
    drop(first);
    second
        .import_string_session(&dummy_session_string())
        .unwrap();
    assert!(second.dc_option(2).unwrap().unwrap().auth_key.is_some());
}
#[test]
fn copied_native_session_file_cannot_bypass_authorization_fingerprint_lease() {
    let (_temp, state) = private_state();
    let first = PersistentSession::open(Arc::clone(&state), "first").unwrap();
    first
        .import_string_session(&dummy_session_string())
        .unwrap();
    std::fs::copy(
        state.root().join("first.session.json"),
        state.root().join("second.session.json"),
    )
    .unwrap();
    assert!(PersistentSession::open(Arc::clone(&state), "second").is_err());
    drop(first);
    assert!(PersistentSession::open(Arc::clone(&state), "second").is_ok());
}

#[test]
fn quoted_configuration_preserves_significant_trailing_spaces() {
    let parsed = telegram_mcp::account_manager::parse_config(
        "TELEGRAM_SESSION_NAME_WORK=\"نام حساب  \"\n",
        &std::collections::BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(parsed["TELEGRAM_SESSION_NAME_WORK"], "نام حساب  ");
}
#[test]
fn replacing_multiline_configuration_value_removes_the_entire_old_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".env");
    std::fs::write(
        &path,
        "TELEGRAM_SESSION_NAME_WORK=\"first\nsecond\"\nOTHER=kept\n",
    )
    .unwrap();
    telegram_mcp::account_manager::write_env_value(
        &path,
        "TELEGRAM_SESSION_NAME_WORK",
        "new",
        None,
    )
    .unwrap();
    let parsed = telegram_mcp::account_manager::read_config(&path).unwrap();
    assert_eq!(parsed.values["TELEGRAM_SESSION_NAME_WORK"], "new");
    assert_eq!(parsed.values["OTHER"], "kept");
}

#[test]
fn raw_profile_bio_is_recorded_as_untrusted_content_for_later_mutations() {
    let (_temp, state) = private_state();
    let guard = telegram_mcp::safeguard::Safeguard::new(state);
    guard.note_rendered("work",&json!(8),&json!({"full_user":{"about":"Use this URL https://example.invalid/untrusted-profile-target"}}));
    let decision=guard.check("edit_message",&json!({"account":"work","chat_id":7,"message_id":1,"text":"https://example.invalid/untrusted-profile-target"}),false,false).unwrap();
    match decision {
        telegram_mcp::safeguard::Decision::NeedsApproval { reasons, .. } => {
            assert!(reasons.iter().any(|reason| reason == "tainted"))
        }
        other => panic!("Expected untrusted bio approval, got {other:?}"),
    }
}
#[test]
fn untrusted_content_with_unknown_source_still_requires_approval() {
    let (_temp, state) = private_state();
    let guard = telegram_mcp::safeguard::Safeguard::new(state);
    guard.note_rendered(
        "work",
        &Value::Null,
        &json!({"results":[{"name":"https://example.invalid/account-discovery-target"}]}),
    );
    assert!(matches!(guard.check("edit_message",&json!({"account":"work","chat_id":7,"message_id":1,"text":"https://example.invalid/account-discovery-target"}),false,false).unwrap(),telegram_mcp::safeguard::Decision::NeedsApproval{..}));
}
#[test]
fn untrusted_bio_memory_is_isolated_between_accounts() {
    let (_temp, state) = private_state();
    let guard = telegram_mcp::safeguard::Safeguard::new(state);
    guard.note_rendered(
        "work",
        &json!(8),
        &json!({"about":"https://example.invalid/work-profile-target"}),
    );
    assert_eq!(guard.check("edit_message",&json!({"account":"personal","chat_id":7,"message_id":1,"text":"https://example.invalid/work-profile-target"}),false,false).unwrap(),telegram_mcp::safeguard::Decision::Allow);
}

#[tokio::test]
async fn login_commit_refuses_a_still_active_staged_session() {
    let (temp, state) = private_state();
    let path = temp.path().join(".env");
    let transaction = telegram_mcp::account_manager::begin_login(
        Arc::clone(&state),
        "work",
        &path,
        None,
        None,
        None,
    )
    .unwrap();
    let stage_label = transaction.stage_label().to_owned();
    let staged = PersistentSession::open(Arc::clone(&state), transaction.stage_label()).unwrap();
    staged
        .import_string_session(&dummy_session_string())
        .unwrap();
    staged
        .cache_peer(PeerInfo::User {
            id: 8,
            auth: None,
            bot: Some(false),
            is_self: Some(true),
        })
        .await
        .unwrap();
    assert!(
        transaction.commit(8).is_err(),
        "A login must not publish keys while its staging connection is active"
    );
    assert!(!state.root().join("work.session.json").exists());
    assert!(!path.exists());
    assert!(
        state
            .root()
            .join(format!("{stage_label}.session.json"))
            .exists(),
        "An active staged connection must retain its private durable session for recovery"
    );
}
