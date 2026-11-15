mod local_support;
use local_support::fixture;
use serde_json::{Value, json};
use std::path::Path;
use telegram_mcp::state::{FileAccess, State, alias_key, identifier_like};

#[test]
fn private_json_roundtrips_unicode_without_replacement() {
    let (_dir, state) = fixture();
    let data = json!({"persian":"سلام دنیا","emoji":"👩🏽‍💻","combining":"é"});
    state.write_json("unicode.json", &data).unwrap();
    assert_eq!(state.read_json("unicode.json", Value::Null).unwrap(), data);
}
#[test]
fn missing_record_returns_its_default() {
    let (_dir, state) = fixture();
    assert_eq!(
        state
            .read_json("missing.json", json!({"fresh":true}))
            .unwrap(),
        json!({"fresh":true})
    );
}
#[test]
fn corrupt_record_is_never_overwritten_by_mutation() {
    let (_dir, state) = fixture();
    let path = state.root().join("corrupt.json");
    std::fs::write(&path, b"{broken").unwrap();
    assert!(
        state
            .update_json("corrupt.json", json!({}), |data| {
                *data = json!({"lost":true});
                Ok(())
            })
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"{broken");
}
#[test]
fn invalid_utf8_fails_without_loss() {
    let (_dir, state) = fixture();
    std::fs::write(state.root().join("bytes.json"), [0xff, 0xfe]).unwrap();
    assert!(state.read_json("bytes.json", Value::Null).is_err());
}
#[test]
fn state_names_cannot_traverse() {
    let (_dir, state) = fixture();
    assert!(state.write_json("../escape.json", &json!({})).is_err());
    assert!(state.read_json("nested/file.json", Value::Null).is_err());
}
#[test]
fn atomic_update_preserves_unrelated_fields() {
    let (_dir, state) = fixture();
    state
        .write_json("record.json", &json!({"a":1,"b":2}))
        .unwrap();
    state
        .update_json("record.json", Value::Null, |data| {
            data["a"] = json!(3);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        state.read_json("record.json", Value::Null).unwrap(),
        json!({"a":3,"b":2})
    );
}
#[test]
fn failed_mutation_keeps_original_record() {
    let (_dir, state) = fixture();
    state.write_json("record.json", &json!({"a":1})).unwrap();
    let result: anyhow::Result<()> = state.update_json("record.json", Value::Null, |data| {
        data["a"] = json!(2);
        anyhow::bail!("refused")
    });
    assert!(result.is_err());
    assert_eq!(
        state.read_json("record.json", Value::Null).unwrap(),
        json!({"a":1})
    );
}
#[test]
fn allowlisted_existing_file_resolves() {
    let (dir, state) = fixture();
    let path = dir.path().join("allowed/photo.png");
    std::fs::write(&path, b"fixture").unwrap();
    assert_eq!(
        state
            .resolve_file(Path::new("photo.png"), FileAccess::Read)
            .unwrap(),
        path.canonicalize().unwrap()
    );
}
#[test]
fn nonexisting_nested_destination_resolves_inside_root() {
    let (dir, state) = fixture();
    assert_eq!(
        state
            .resolve_file(Path::new("downloads/new/file.json"), FileAccess::Write)
            .unwrap(),
        dir.path().join("allowed/downloads/new/file.json")
    );
}
#[test]
fn nonexisting_read_is_rejected() {
    let (_dir, state) = fixture();
    assert!(
        state
            .resolve_file(Path::new("missing.txt"), FileAccess::Read)
            .is_err()
    );
}
#[test]
fn parent_traversal_is_rejected_before_access() {
    let (_dir, state) = fixture();
    assert!(
        state
            .resolve_file(Path::new("nested/../file"), FileAccess::Write)
            .is_err()
    );
}
#[test]
fn absolute_outside_destination_is_rejected() {
    let (dir, state) = fixture();
    assert!(
        state
            .resolve_file(&dir.path().join("outside/file"), FileAccess::Write)
            .is_err()
    );
}
#[test]
fn private_state_is_protected_even_under_broad_root() {
    let (dir, _state) = fixture();
    let state = State::at_with_installation(
        dir.path().join("private"),
        vec![dir.path().to_owned()],
        dir.path().join("installation"),
    )
    .unwrap();
    assert!(
        state
            .resolve_file(&state.root().join("session.json"), FileAccess::Write)
            .is_err()
    );
}
#[test]
fn installation_is_protected_even_under_broad_root() {
    let (dir, _state) = fixture();
    let state = State::at_with_installation(
        dir.path().join("private"),
        vec![dir.path().to_owned()],
        dir.path().join("installation"),
    )
    .unwrap();
    assert!(
        state
            .resolve_file(
                &dir.path().join("installation/Cargo.toml"),
                FileAccess::Write
            )
            .is_err()
    );
}
#[test]
fn installation_files_remain_accessible() {
    let (dir, _state) = fixture();
    let state = State::at_with_installation(
        dir.path().join("private"),
        vec![dir.path().join("installation/files")],
        dir.path().join("installation"),
    )
    .unwrap();
    assert!(
        state
            .resolve_file(Path::new("outbox/upload.txt"), FileAccess::Write)
            .is_ok()
    );
}
#[test]
fn private_dotenv_is_refused_inside_allowed_root() {
    let (_dir, state) = fixture();
    assert!(
        state
            .resolve_file(Path::new(".env"), FileAccess::Write)
            .is_err()
    );
}
#[test]
fn private_repo_metadata_is_refused_inside_allowed_root() {
    let (_dir, state) = fixture();
    assert!(
        state
            .resolve_file(Path::new(".git/config"), FileAccess::Write)
            .is_err()
    );
}
#[test]
fn directory_cannot_be_read_as_file() {
    let (_dir, state) = fixture();
    assert!(
        state
            .resolve_file(Path::new("."), FileAccess::Read)
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn symlink_escape_for_existing_and_new_files_is_rejected() {
    let (dir, state) = fixture();
    std::fs::create_dir(dir.path().join("outside")).unwrap();
    std::fs::write(dir.path().join("outside/secret.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside"), dir.path().join("allowed/link"))
        .unwrap();
    assert!(
        state
            .resolve_file(Path::new("link/secret.txt"), FileAccess::Read)
            .is_err()
    );
    assert!(
        state
            .resolve_file(Path::new("link/new.txt"), FileAccess::Write)
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn private_state_files_have_owner_only_modes() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, state) = fixture();
    state.write_json("permissions.json", &json!({})).unwrap();
    assert_eq!(
        std::fs::metadata(state.root())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(state.root().join("permissions.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[cfg(unix)]
#[test]
fn symlink_state_record_is_never_followed() {
    let (dir, state) = fixture();
    let outside = dir.path().join("outside.json");
    std::fs::write(&outside, b"{\"safe\":true}").unwrap();
    std::os::unix::fs::symlink(&outside, state.root().join("record.json")).unwrap();
    assert!(state.read_json("record.json", Value::Null).is_err());
    assert!(
        state
            .write_json("record.json", &json!({"safe":false}))
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "{\"safe\":true}");
}

#[test]
fn alias_normalization_collapses_unicode_whitespace() {
    assert_eq!(alias_key("  دوست\tقدیمی  "), "دوست قدیمی");
}
#[test]
fn real_identifiers_cannot_be_shadowed() {
    for alias in [
        "me",
        "self",
        "@somebody",
        "12345",
        "+1 (555) 123-4567",
        "actual_username",
        "t.me/person",
    ] {
        assert!(identifier_like(alias), "{alias}");
    }
}
#[test]
fn natural_language_alias_is_account_scoped() {
    let (_dir, state) = fixture();
    let peer = json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"});
    state
        .alias_set("work", "close friend", peer.clone(), false)
        .unwrap();
    assert_eq!(
        state.alias_lookup("work", "CLOSE FRIEND").unwrap().unwrap()["peer"],
        peer
    );
    assert!(
        state
            .alias_lookup("personal", "close friend")
            .unwrap()
            .is_none()
    );
}
#[test]
fn alias_retarget_needs_explicit_replace() {
    let (_dir, state) = fixture();
    state
        .alias_set("work", "close friend", json!({"user_id":"1"}), false)
        .unwrap();
    assert!(
        state
            .alias_set("work", "close friend", json!({"user_id":"2"}), false)
            .is_err()
    );
    assert_eq!(
        state.alias_lookup("work", "close friend").unwrap().unwrap()["peer"]["user_id"],
        "1"
    );
    state
        .alias_set("work", "close friend", json!({"user_id":"2"}), true)
        .unwrap();
    assert_eq!(
        state.alias_lookup("work", "close friend").unwrap().unwrap()["peer"]["user_id"],
        "2"
    );
}
#[test]
fn alias_deletion_is_exact_and_scoped() {
    let (_dir, state) = fixture();
    state
        .alias_set("work", "close friend", json!({"id":1}), false)
        .unwrap();
    state
        .alias_set("personal", "close friend", json!({"id":2}), false)
        .unwrap();
    assert!(!state.alias_remove("work", "close friends").unwrap());
    assert!(state.alias_remove("work", "close friend").unwrap());
    assert!(
        state
            .alias_lookup("personal", "close friend")
            .unwrap()
            .is_some()
    );
}
#[test]
fn corrupt_alias_memory_survives_failed_add() {
    let (_dir, state) = fixture();
    std::fs::write(state.root().join("aliases-v2.json"), b"broken").unwrap();
    assert!(
        state
            .alias_set("work", "close friend", json!({}), false)
            .is_err()
    );
    assert_eq!(
        std::fs::read(state.root().join("aliases-v2.json")).unwrap(),
        b"broken"
    );
}

#[test]
fn ghost_defaults_on() {
    let (_dir, state) = fixture();
    assert_eq!(state.ghost_mode(None, None)["effective"], true);
}
#[test]
fn ghost_chat_overrides_account_and_default() {
    let (_dir, state) = fixture();
    state.set_ghost_mode(false, None, None).unwrap();
    state.set_ghost_mode(true, Some("work"), None).unwrap();
    state
        .set_ghost_mode(false, Some("work"), Some(&json!(42)))
        .unwrap();
    assert_eq!(
        state.ghost_mode(Some("work"), Some(&json!(42)))["effective"],
        false
    );
    assert_eq!(
        state.ghost_mode(Some("work"), Some(&json!(43)))["effective"],
        true
    );
    assert_eq!(state.ghost_mode(Some("personal"), None)["effective"], false);
}
#[test]
fn corrupt_ghost_settings_fail_closed() {
    let (_dir, state) = fixture();
    std::fs::write(state.root().join("ghost.json"), b"not-json").unwrap();
    assert_eq!(state.ghost_mode(Some("work"), None)["effective"], true);
    assert!(!state.ghost_mode(None, None)["error"].is_null());
    assert!(state.set_ghost_mode(false, None, None).is_err());
}
#[test]
fn chat_ghost_setting_requires_account() {
    let (_dir, state) = fixture();
    assert!(state.set_ghost_mode(false, None, Some(&json!(42))).is_err());
}
