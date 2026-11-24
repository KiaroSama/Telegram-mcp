use super::*;
use std::{fs, panic::AssertUnwindSafe};
#[cfg(windows)]
#[path = "../common/windows_acl.rs"]
mod windows_barrier_acl;

fn fixture() -> (tempfile::TempDir, Arc<State>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let state = State::at(root.path().join("state"), vec![files]).unwrap();
    (root, Arc::new(state))
}

#[test]
fn directory_barrier_confirms_namespace_changes_and_refuses_missing_or_regular_paths() {
    use crate::state::storage::sync_directory;
    use std::io::Write;
    let (root, _state) = fixture();
    let parent = root.path().join("barrier");
    fs::create_dir(&parent).unwrap();
    sync_directory(&parent).unwrap();
    let old = parent.join("old");
    let new = parent.join("new");
    let mut file = fs::File::create(&old).unwrap();
    file.write_all(b"namespace-canary").unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::rename(&old, &new).unwrap();
    sync_directory(&parent).unwrap();
    assert_eq!(fs::read(&new).unwrap(), b"namespace-canary");
    fs::remove_file(&new).unwrap();
    sync_directory(&parent).unwrap();
    fs::remove_dir(&parent).unwrap();
    assert_eq!(
        sync_directory(&parent).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    fs::write(&parent, b"preserve-regular-file").unwrap();
    assert_eq!(
        sync_directory(&parent).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(fs::read(parent).unwrap(), b"preserve-regular-file");
}

#[cfg(windows)]
#[test]
fn denied_directory_barrier_refuses_new_account_and_state_publication_before_changes() {
    use crate::account_manager::{begin_login, rename_configured_account, write_env_value};
    use crate::state::storage::sync_directory;
    let (root, state) = fixture();
    let parent = root.path().join("configuration");
    fs::create_dir(&parent).unwrap();
    let env = parent.join("selected.env");
    let original = "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=keep\n";
    fs::write(&env, original).unwrap();
    let native = authorized_account_snapshot();
    state.write_json("work.session.json", &native).unwrap();
    let mut denied = windows_barrier_acl::ReadDenied::new(&parent).unwrap();
    let before = denied.current_acl().unwrap();
    assert_eq!(
        sync_directory(&parent).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(write_env_value(&env, "TELEGRAM_SESSION_NAME_NEW", "new", None).is_err());
    assert!(begin_login(state.clone(), "new", &env, None, None, None).is_err());
    assert!(
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .is_err()
    );
    assert_eq!(denied.current_acl().unwrap(), before);
    denied.restore().unwrap();
    assert_eq!(denied.current_acl().unwrap(), denied.original_acl());
    assert_eq!(fs::read_to_string(&env).unwrap(), original);
    assert_eq!(
        fs::read_dir(&parent).unwrap().count(),
        1,
        "Admission created a lock, backup, or pending configuration record"
    );
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    assert!(!state.root().join("office.session.json").exists());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    let mut denied = windows_barrier_acl::ReadDenied::new(state.root()).unwrap();
    let before = denied.current_acl().unwrap();
    assert!(
        state
            .write_json("new-record.json", &json!({"never":"published"}))
            .is_err()
    );
    assert_eq!(denied.current_acl().unwrap(), before);
    denied.restore().unwrap();
    assert_eq!(denied.current_acl().unwrap(), denied.original_acl());
    assert!(!state.root().join("new-record.json").exists());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    sync_directory(state.root()).unwrap();
}

fn authorized_account_snapshot() -> Value {
    account_snapshot(42, 7)
}

fn account_snapshot(user: i64, key: u8) -> Value {
    let mut data = grammers_session::SessionData::default();
    data.dc_options.get_mut(&data.home_dc).unwrap().auth_key = Some([key; 256]);
    json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[grammers_session::types::PeerInfo::User{id:user,auth:None,bot:Some(false),is_self:Some(true)}],"updates_state":data.updates_state})
}

#[test]
fn malformed_alias_account_buckets_return_errors_without_panicking_or_replacing_bytes() {
    let (_root, state) = fixture();
    for bucket in [
        json!("invalid"),
        json!([]),
        json!(false),
        json!(7),
        Value::Null,
    ] {
        state
            .write_json(
                "aliases-v2.json",
                &json!({"accounts":{"work":bucket},"owner_note":"preserve"}),
            )
            .unwrap();
        let path = state.root().join("aliases-v2.json");
        let before = fs::read(&path).unwrap();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            state.alias_set(
                "work",
                "close friend",
                json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}),
                false,
            )
        }));
        assert!(result.is_ok(), "A malformed alias account bucket panicked");
        assert!(result.unwrap().is_err());
        assert!(state.alias_remove("work", "close friend").is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn malformed_approval_store_mutations_return_errors_and_preserve_original_bytes() {
    let (_root, state) = fixture();
    for value in [
        json!([]),
        json!("invalid"),
        json!(false),
        json!(7),
        Value::Null,
    ] {
        state.write_json("always-approvals.json", &value).unwrap();
        let path = state.root().join("always-approvals.json");
        let before = fs::read(&path).unwrap();
        for operation in 0..3 {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| match operation {
                0 => state
                    .grant_approval("work", "send_message", &json!(42))
                    .map(|_| ()),
                1 => state
                    .revoke_approval("work", "send_message", &json!(42))
                    .map(|_| ()),
                _ => state
                    .revoke_folder(state.file_roots()[0].to_str().unwrap())
                    .map(|_| ()),
            }));
            assert!(result.is_ok(), "Malformed approval storage panicked");
            assert!(result.unwrap().is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert!(!state.approval_is_granted("work", "send_message", &json!(42)));
    }
}

#[test]
fn malformed_nested_ghost_maps_refuse_mutations_without_panics_or_replacement() {
    let (_root, state) = fixture();
    for value in [
        json!({"accounts":[]}),
        json!({"accounts":"invalid"}),
        json!({"chats":[]}),
        json!({"chats":{"work":false}}),
    ] {
        state.write_json("ghost.json", &value).unwrap();
        let path = state.root().join("ghost.json");
        let before = fs::read(&path).unwrap();
        for scope in [1, 2, 0] {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| match scope {
                0 => state.set_ghost_mode(false, None, None),
                1 => state.set_ghost_mode(false, Some("work"), None),
                _ => state.set_ghost_mode(false, Some("work"), Some(&json!(42))),
            }));
            assert!(result.is_ok(), "Malformed nested ghost settings panicked");
            assert!(result.unwrap().is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }
}

#[test]
fn state_json_byte_budget_includes_its_newline_and_preserves_old_bytes_on_refusal() {
    let (_root, state) = fixture();
    let limit = 16 * 1024 * 1024;
    state
        .write_json("boundary.json", &json!({"before":true}))
        .unwrap();
    let path = state.root().join("boundary.json");
    let before = fs::read(&path).unwrap();
    let rejected = Value::String("x".repeat(limit - 2));
    assert_eq!(serde_json::to_vec_pretty(&rejected).unwrap().len(), limit);
    assert!(state.write_json("boundary.json", &rejected).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let accepted = Value::String("x".repeat(limit - 3));
    state.write_json("boundary.json", &accepted).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), limit as u64);
    assert_eq!(
        state.read_json("boundary.json", Value::Null).unwrap(),
        accepted
    );
}

#[test]
fn state_writer_waits_for_a_real_contended_lock_then_commits() {
    let (_root, state) = fixture();
    let held = state.state_lock().unwrap();
    let writer = Arc::clone(&state);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let probe = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(writer.root().join("state.lock"))
            .unwrap();
        let contention = fs2::FileExt::try_lock_exclusive(&probe).unwrap_err();
        assert_eq!(
            contention.raw_os_error(),
            fs2::lock_contended_error().raw_os_error()
        );
        ready_tx.send(()).unwrap();
        done_tx
            .send(writer.write_json("after-contention.json", &json!({"committed":true})))
            .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    drop(held);
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
    assert_eq!(
        state
            .read_json("after-contention.json", Value::Null)
            .unwrap()["committed"],
        true
    );
}

#[test]
fn a_post_replace_sync_failure_is_distinct_from_an_unchanged_precommit_failure() {
    use super::super::storage::{DurabilityUncertain, replace_and_sync};
    use anyhow::Context;
    let (root, _state) = fixture();
    let destination = root.path().join("committed.json");
    let temporary = root.path().join("pending.json");
    fs::write(&destination, b"previous").unwrap();
    fs::write(&temporary, b"replacement").unwrap();
    let error = replace_and_sync(&temporary, &destination, || {
        Err(std::io::Error::other("simulated directory sync failure"))
    })
    .context("session persistence failed")
    .unwrap_err();
    assert!(error.downcast_ref::<DurabilityUncertain>().is_some());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!temporary.exists());
    let before = fs::read(&destination).unwrap();
    let error = replace_and_sync(&temporary, &destination, || Ok(())).unwrap_err();
    assert!(error.downcast_ref::<DurabilityUncertain>().is_none());
    assert_eq!(fs::read(destination).unwrap(), before);
}

// Keep the fully qualified names used by the CI test inventory.
include!("state_review/rename_receipts.rs");
include!("state_review/publication.rs");
include!("state_review/incoming_feed.rs");
