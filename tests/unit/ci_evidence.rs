use super::*;

#[test]
fn junit_records_actual_cases_and_failure_outcomes() {
    let report = r#"<testsuites><testsuite name="telegram-mcp::native" tests="2"><testcase name="ok"/><testcase name="failed"><failure message="private"/></testcase></testsuite></testsuites>"#;
    let (cases, executed, failed, skipped) = parse_report(report).unwrap();
    assert_eq!(
        cases,
        vec!["telegram-mcp::native::failed", "telegram-mcp::native::ok"]
    );
    assert_eq!(executed, cases);
    assert_eq!((failed, skipped), (1, 0));
}
#[test]
fn junit_retains_skipped_case_without_claiming_execution() {
    let report = r#"<testsuites><testsuite name="native" tests="2"><testcase name="ran"/><testcase name="platform"><skipped/></testcase></testsuite></testsuites>"#;
    let (cases, executed, failed, skipped) = parse_report(report).unwrap();
    assert_eq!(cases.len(), 2);
    assert_eq!(executed, vec!["native::ran"]);
    assert_eq!((failed, skipped), (0, 1));
}
#[test]
fn junit_rejects_declared_count_disagreeing_with_cases() {
    assert!(
        parse_report(r#"<testsuite name="native" tests="2"><testcase name="one"/></testsuite>"#)
            .is_err()
    );
}
#[test]
fn junit_rejects_duplicate_case_identity() {
    assert!(parse_report(r#"<testsuite name="native" tests="2"><testcase name="one"/><testcase name="one"/></testsuite>"#).is_err());
}
#[test]
fn junit_rejects_invalid_xml_and_empty_reports() {
    assert!(parse_report("<testsuite>").is_err());
    assert!(parse_report("<testsuites/>").is_err());
    assert!(parse_report(r#"<testsuite name="native" tests="0"/>"#).is_err());
}
#[test]
fn release_package_contains_all_native_clis_and_actual_binary_hashes() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("release");
    fs::create_dir(&source).unwrap();
    for (name, content) in [
        ("telegram-mcp", "server-fixture"),
        ("emoji-studio", "emoji-fixture"),
        ("operator-plugin", "operator-fixture"),
    ] {
        fs::write(source.join(name), content).unwrap();
    }
    for name in ["LICENSE", "NOTICE", "README.md"] {
        fs::write(root.path().join(name), name).unwrap();
    }
    let output = root.path().join("package");
    package_files(
        &source,
        &output,
        root.path(),
        "",
        serde_json::json!({"commit":"synthetic-build"}),
    )
    .unwrap();
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("build.json")).unwrap()).unwrap();
    assert_eq!(metadata["binaries"].as_object().unwrap().len(), 3);
    for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
        assert_eq!(
            metadata["binaries"][name]["sha256"],
            process::digest(&output.join(name)).unwrap()
        );
    }
}
#[test]
fn release_package_refuses_missing_helper_before_creating_output() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("release");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("telegram-mcp.exe"), "synthetic-server").unwrap();
    let output = root.path().join("package");
    assert!(package_files(&source, &output, root.path(), ".exe", serde_json::json!({})).is_err());
    assert!(!output.exists());
}
#[test]
fn release_package_cannot_upload_stale_or_owner_files() {
    let root = tempfile::TempDir::new().unwrap();
    let output = root.path().join("package");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("owner.txt"), "preserve").unwrap();
    assert!(package_files(root.path(), &output, root.path(), "", serde_json::json!({})).is_err());
    assert_eq!(
        fs::read_to_string(output.join("owner.txt")).unwrap(),
        "preserve"
    );
}
