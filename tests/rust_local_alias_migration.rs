mod local_support;
use local_support::fixture;
use serde_json::json;

#[test]
fn legacy_scoped_aliases_migrate_without_crossing_accounts_or_rewriting_the_source() {
    let (_dir, state) = fixture();
    state.write_json("aliases.json",&json!({"work\nدوست قدیمی":{"id":42,"name":"نام اول","account":"work"},"personal\nدوست قدیمی":{"id":99,"name":"نام دوم","account":"personal"}})).unwrap();
    let original = std::fs::read(state.root().join("aliases.json")).unwrap();
    assert_eq!(
        state.alias_lookup("work", "دوست قدیمی").unwrap().unwrap()["peer"],
        "42"
    );
    assert_eq!(
        state
            .alias_lookup("personal", "دوست قدیمی")
            .unwrap()
            .unwrap()["peer"],
        "99"
    );
    assert!(state.root().join("aliases-v2.json").exists());
    assert_eq!(
        std::fs::read(state.root().join("aliases.json")).unwrap(),
        original
    );
}

#[test]
fn historic_scalar_alias_rows_and_unicode_normalization_are_preserved() {
    let (_dir, state) = fixture();
    state
        .write_json(
            "aliases.json",
            &json!({"work\ncafé friend":42,"work\nПётр Первый":{"id":"123","name":"Пётр"}}),
        )
        .unwrap();
    assert_eq!(
        state.alias_lookup("work", "café friend").unwrap().unwrap()["id"],
        "42"
    );
    assert_eq!(
        state.alias_lookup("work", "петр  первый").unwrap().unwrap()["id"],
        "123"
    );
}

#[test]
fn malformed_legacy_alias_data_is_preserved_instead_of_overwritten_by_a_new_alias() {
    let (_dir, state) = fixture();
    std::fs::write(state.root().join("aliases.json"), b"not-json").unwrap();
    assert!(
        state
            .alias_set("work", "close friend", json!({"user_id":"42"}), false)
            .is_err()
    );
    assert_eq!(
        std::fs::read(state.root().join("aliases.json")).unwrap(),
        b"not-json"
    );
    assert!(!state.root().join("aliases-v2.json").exists());
}

#[test]
fn conflicting_legacy_account_scopes_do_not_become_silent_recipients() {
    let (_dir, state) = fixture();
    state
        .write_json(
            "aliases.json",
            &json!({"work\nclose friend":{"id":42,"account":"personal"}}),
        )
        .unwrap();
    assert!(state.alias_lookup("work", "close friend").is_err());
    assert!(!state.root().join("aliases-v2.json").exists());
}

#[test]
fn inflected_words_and_reordered_tokens_are_suggestions_only() {
    let (_dir, state) = fixture();
    state
        .alias_set(
            "work",
            "андрей бекендер",
            json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}),
            false,
        )
        .unwrap();
    for query in ["андрею бекендеру", "бекендер андрей", "бекендеру"]
    {
        assert!(state.alias_lookup("work", query).unwrap().is_none());
        let matches = state.alias_matches("work", query).unwrap();
        assert_eq!(matches[0]["alias"], "андрей бекендер");
        assert_eq!(matches[0]["match"], "similar");
    }
}

#[test]
fn a_query_token_cannot_claim_the_same_stored_word_twice() {
    let (_dir, state) = fixture();
    state
        .alias_set(
            "work",
            "андрей",
            json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}),
            false,
        )
        .unwrap();
    assert!(
        state
            .alias_matches("work", "андрей андреев")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn exact_alias_match_wins_over_other_similar_suggestions() {
    let (_dir, state) = fixture();
    state
        .alias_set("work", "андрей", json!({"user_id":"42"}), false)
        .unwrap();
    state
        .alias_set("work", "андрей бекендер", json!({"user_id":"43"}), false)
        .unwrap();
    let matches = state.alias_matches("work", "андрей").unwrap();
    assert_eq!(matches.as_array().unwrap().len(), 1);
    assert_eq!(matches[0]["match"], "exact");
}

#[test]
fn alias_suggestions_use_marked_ids_for_basic_groups_and_channels() {
    let (_dir, state) = fixture();
    state
        .alias_set(
            "work",
            "basic group",
            json!({"_":"inputPeerChat","chat_id":"42"}),
            false,
        )
        .unwrap();
    state
        .alias_set(
            "work",
            "broadcast channel",
            json!({"_":"inputPeerChannel","channel_id":"43","access_hash":"7"}),
            false,
        )
        .unwrap();
    assert_eq!(
        state.alias_matches("work", "basic group").unwrap()[0]["id"],
        "-42"
    );
    assert_eq!(
        state.alias_matches("work", "broadcast channel").unwrap()[0]["id"],
        "-1000000000043"
    );
}

#[test]
fn refreshing_the_same_contacts_access_hash_is_not_a_recipient_change() {
    let (_dir, state) = fixture();
    state
        .alias_set(
            "work",
            "close friend",
            json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}),
            false,
        )
        .unwrap();
    state
        .alias_set(
            "work",
            "close friend",
            json!({"_":"inputPeerUser","user_id":"42","access_hash":"8"}),
            false,
        )
        .unwrap();
    assert_eq!(
        state.alias_lookup("work", "close friend").unwrap().unwrap()["peer"]["access_hash"],
        "8"
    );
}
