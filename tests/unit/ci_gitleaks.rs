use super::*;
#[test]
fn git_revision_rejects_option_injection_and_missing_hash() {
    assert!(scope::revision(Some("--all")).is_err());
    assert!(scope::revision(Some("aabb")).is_err());
    assert!(scope::revision(None).is_err());
    let valid = "ab".repeat(20);
    assert_eq!(scope::revision(Some(&valid)).unwrap(), valid);
}
#[test]
fn scanner_summary_escapes_markup_and_table_delimiters() {
    assert_eq!(
        safe(&Value::String("<file>|\n&".into())),
        "&lt;file&gt;&#124; &amp;"
    );
}
