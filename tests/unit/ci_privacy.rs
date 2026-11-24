use super::*;
#[test]
fn credential_in_example_is_not_exempt_by_path() {
    let candidate = format!("api_hash={}\n", "a1".repeat(16));
    assert_eq!(
        problems(&candidate, &rules().unwrap()).unwrap()[0].1,
        "Telegram API hash"
    );
}
#[test]
fn known_placeholder_and_commit_hash_are_not_credentials() {
    let text = format!(
        "{}\n{}\n{}",
        PLACEHOLDERS[0],
        "ab".repeat(20),
        "cd".repeat(32)
    );
    assert!(problems(&text, &rules().unwrap()).unwrap().is_empty());
}
#[test]
fn ignored_placeholder_does_not_hide_an_adjacent_credential() {
    for separator in ["\n", " ", ":", "•"] {
        let candidate = format!("{}{separator}{}", PLACEHOLDERS[0], "f".repeat(32));
        let findings = problems(&candidate, &rules().unwrap()).unwrap();
        assert_eq!(findings.len(), 1, "Adjacent candidate escaped detection");
        assert_eq!(findings[0].1, "Telegram API hash");
        assert_eq!(findings[0].0, if separator == "\n" { 2 } else { 1 });
    }
}
#[test]
fn protects_accounts_but_allows_public_plugin_manifests() {
    assert!(protected(".personal/12345/identity.json"));
    assert!(protected(".env.private"));
    assert!(protected("a/b.session"));
    assert!(!protected(".env.example"));
    assert!(!protected(".codex-plugin/plugin.json"));
}
#[test]
fn allows_only_the_root_codex_marketplace_manifest() {
    assert!(!protected(".agents/plugins/marketplace.json"));
    assert!(!protected(r".agents\plugins\marketplace.json"));
    assert!(protected(".agents/notes.md"));
    assert!(protected("sub/.agents/plugins/marketplace.json"));
}
#[test]
fn matches_real_home_names_and_allows_generic_examples() {
    assert!(
        !problems(
            &["/home/", "private-account/data"].concat(),
            &rules().unwrap()
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        problems("/home/runner/data", &rules().unwrap())
            .unwrap()
            .is_empty()
    );
}
