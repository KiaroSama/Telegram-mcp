mod local_support;
use local_support::fixture;
use serde_json::{Value, json};

#[test]
fn renaming_an_account_without_aliases_preserves_empty_namespaces_and_allows_new_aliases() {
    let (_dir, state) = fixture();
    state
        .write_json("aliases-v2.json", &json!({"accounts":{}}))
        .unwrap();
    rename_account(&state, "work", "personal").unwrap();
    assert_eq!(
        state.read_json("aliases-v2.json", Value::Null).unwrap(),
        json!({"accounts":{}})
    );
    assert_eq!(state.aliases("personal").unwrap(), json!({}));
    state
        .alias_set("personal", "close friend", json!({"user_id":"42"}), false)
        .unwrap();
    assert!(
        state
            .alias_lookup("personal", "close friend")
            .unwrap()
            .is_some()
    );
}
use telegram_mcp::account_manager::{recover_rename, rename_account};

#[test]
fn renamed_legacy_alias_records_remain_resolvable_in_their_new_scope() {
    let (_dir, state) = fixture();
    state.write_json("aliases.json",&json!({"work\nclose friend":{"id":42,"name":"Original","account":"work"},"personal\nother friend":{"id":99,"name":"Other","account":"personal"}})).unwrap();
    rename_account(&state, "work", "new").unwrap();
    assert_eq!(
        state.alias_lookup("new", "close friend").unwrap().unwrap()["id"],
        "42"
    );
    assert!(
        state
            .alias_lookup("work", "close friend")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state
            .alias_lookup("personal", "other friend")
            .unwrap()
            .unwrap()["id"],
        "99"
    );
}

#[test]
fn rename_updates_every_proxy_membership_and_preserves_unrelated_shared_membership() {
    let (_dir, state) = fixture();
    state.write_json("proxies-v1.json",&json!({"a":{"pool":"work","pools":["work","shared"]},"b":{"pool":"shared","pools":["shared","work"]}})).unwrap();
    rename_account(&state, "work", "new").unwrap();
    let pools = state.read_json("proxies-v1.json", Value::Null).unwrap();
    assert_eq!(pools["a"]["pool"], "new");
    assert_eq!(pools["a"]["pools"], json!(["new", "shared"]));
    assert_eq!(pools["b"]["pool"], "shared");
    assert_eq!(pools["b"]["pools"], json!(["shared", "new"]));
}

#[test]
fn account_rename_moves_authorization_binding_grants_routes_and_invalidates_pending_requests() {
    let (_dir, state) = fixture();
    let binding = "TELEGRAM_SESSION_STRING_WORK:existing-source-digest";
    state
        .write_json(
            "work.session.json",
            &json!({"authorization_source":binding,"private_fixture":true}),
        )
        .unwrap();
    state
        .write_json(
            "aliases-v2.json",
            &json!({"accounts":{"work":{"close friend":{"peer":"42","account":"work"}}}}),
        )
        .unwrap();
    state
        .write_json(
            "sent-recipients.json",
            &json!({"work":["42"],"personal":["99"]}),
        )
        .unwrap();
    state
        .write_json(
            "always-approvals.json",
            &json!({"grants":[["work","delete_message","42"]],"folders":[]}),
        )
        .unwrap();
    state
        .write_json(
            "proxy-pool.json",
            &json!({"accounts":{"work":["private"]},"routes":{"work":"route"}}),
        )
        .unwrap();
    state
        .write_json(
            "approval-messages.json",
            &json!({"accounts":{"work":{"last_id":7}},"routes":{"work":"owner"}}),
        )
        .unwrap();
    for name in ["approval-pending.json", "approval-grants.json"] {
        state
            .write_json(name, &json!({"exact-old-arguments":{"valid":true}}))
            .unwrap();
    }
    state.set_ghost_mode(false, Some("work"), None).unwrap();
    state
        .set_ghost_mode(true, Some("work"), Some(&json!(42)))
        .unwrap();
    rename_account(&state, "work", "default").unwrap();
    assert_eq!(
        state
            .read_json("default.session.json", Value::Null)
            .unwrap()["authorization_source"],
        "TELEGRAM_SESSION_STRING:existing-source-digest"
    );
    assert_eq!(
        state.read_json("aliases-v2.json", Value::Null).unwrap()["accounts"]["default"]["close friend"]
            ["account"],
        "default"
    );
    assert!(state.approval_is_granted("default", "delete_message", &json!(42)));
    assert!(!state.approval_is_granted("work", "delete_message", &json!(42)));
    assert_eq!(state.ghost_mode(Some("default"), None)["effective"], false);
    assert_eq!(
        state.ghost_mode(Some("default"), Some(&json!(42)))["effective"],
        true
    );
    assert_eq!(
        state.read_json("proxy-pool.json", Value::Null).unwrap()["routes"]["default"],
        "route"
    );
    assert_eq!(
        state
            .read_json("approval-messages.json", Value::Null)
            .unwrap()["accounts"]["default"]["last_id"],
        7
    );
    assert_eq!(
        state
            .read_json("sent-recipients.json", Value::Null)
            .unwrap()["personal"],
        json!(["99"])
    );
    for name in ["approval-pending.json", "approval-grants.json"] {
        assert_eq!(state.read_json(name, Value::Null).unwrap(), json!({}));
    }
    rename_account(&state, "default", "new").unwrap();
    assert_eq!(
        state.read_json("new.session.json", Value::Null).unwrap()["authorization_source"],
        "TELEGRAM_SESSION_STRING_NEW:existing-source-digest"
    );
    assert!(rename_account(&state, "new", "new").unwrap().is_empty());
}

#[test]
fn destination_metadata_conflicts_refuse_before_native_session_or_source_mutation() {
    for (name, metadata) in [
        ("ghost.json", json!({"accounts":{"work":false,"new":true}})),
        (
            "aliases.json",
            json!({"work\nfriend":42,"new\nexisting":99}),
        ),
        (
            "always-approvals.json",
            json!({"grants":[["work","delete_message","42"],["new","delete_message","99"]]}),
        ),
        (
            "proxies-v1.json",
            json!({"a":{"pool":"work","pools":["work"]},"b":{"pool":"shared","pools":["shared","new"]}}),
        ),
    ] {
        let (_dir, state) = fixture();
        state
            .write_json("work.session.json", &json!({"private_fixture":true}))
            .unwrap();
        state.write_json(name, &metadata).unwrap();
        let before = std::fs::read(state.root().join(name)).unwrap();
        assert!(rename_account(&state, "work", "new").is_err(), "{name}");
        assert_eq!(std::fs::read(state.root().join(name)).unwrap(), before);
        assert!(state.root().join("work.session.json").exists());
        assert!(!state.root().join("new.session.json").exists());
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
    }
}

#[test]
fn recovery_preserves_conflicting_files_and_refuses_traversal_in_a_private_journal() {
    let (dir, state) = fixture();
    state
        .write_json("work.session.json", &json!({"source":true}))
        .unwrap();
    state
        .write_json("new.session.json", &json!({"target":true}))
        .unwrap();
    state
        .write_json(
            "account-rename-transaction.json",
            &json!({"files":[["work.session.json","new.session.json"]],"rewrites":[]}),
        )
        .unwrap();
    assert!(recover_rename(&state).is_err());
    assert!(state.root().join("work.session.json").exists());
    assert!(state.root().join("new.session.json").exists());
    assert!(
        state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    let outside = dir.path().join("outside.json");
    std::fs::write(&outside, "preserve").unwrap();
    // Replace the synthetic journal outside normal writers to model corruption;
    // guarded state writes intentionally refuse any pending rename transaction.
    std::fs::write(
        state.root().join("account-rename-transaction.json"),
        serde_json::to_vec(
            &json!({"files":[["../outside.json","new.session.json"]],"rewrites":[]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(recover_rename(&state).is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "preserve");
}

#[cfg(unix)]
#[test]
fn renaming_refuses_symlinked_lease_and_secret_directory_escape() {
    use std::os::unix::fs::symlink;
    let (dir, state) = fixture();
    let external = dir.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("work"), "untouched").unwrap();
    symlink(&external, state.root().join("secret-chats")).unwrap();
    state
        .write_json("work.session.json", &json!({"private":true}))
        .unwrap();
    assert!(rename_account(&state, "work", "new").is_err());
    assert_eq!(
        std::fs::read_to_string(external.join("work")).unwrap(),
        "untouched"
    );
    std::fs::remove_file(state.root().join("secret-chats")).unwrap();
    std::fs::remove_file(state.root().join("work.session.lock")).unwrap();
    symlink(
        external.join("work"),
        state.root().join("work.session.lock"),
    )
    .unwrap();
    assert!(rename_account(&state, "work", "new").is_err());
    assert!(state.root().join("work.session.json").exists());
}
