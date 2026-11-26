use super::*;

fn reviewed_scope() -> CoverageScope {
    CoverageScope {
        schema_version: 1,
        legacy_commit: "b".repeat(40),
        legacy_pyproject_sha256: "c".repeat(64),
        floor_percent: 85,
        include: BTreeMap::from([(
            "src/main.rs".into(),
            "Legacy measured main entry point".into(),
        )]),
        exclude: BTreeMap::new(),
    }
}

#[test]
fn coverage_refuses_stale_object_lines_outside_current_source() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {\n}\n").unwrap();
    let scope = reviewed_scope();
    scope.validate_source_tree(root.path()).unwrap();
    let mut lengths = BTreeMap::new();
    for line in [1, 2] {
        scope
            .validate_source_line(root.path(), "src/main.rs", line, &mut lengths)
            .unwrap();
    }
    let stale =
        parse_lcov_report("SF:src/main.rs\nDA:3,99\nend_of_record\n", "telegram-mcp").unwrap();
    let ((path, line), _) = stale.first_key_value().unwrap();
    assert!(
        scope
            .validate_source_line(root.path(), path, *line, &mut lengths)
            .is_err(),
        "Executed stale object-map lines beyond the actual source entered a coverage result"
    );
    assert!(
        scope
            .validate_source_line(root.path(), "src/main.rs", 0, &mut lengths)
            .is_err()
    );
    let error = scope
        .validate_source_line(root.path(), "src/unreviewed.rs", 1, &mut lengths)
        .unwrap_err();
    assert!(error.to_string().contains("unreviewed"));
    let mut full_scope = scope;
    fs::write(root.path().join("src/adapter.rs"), "fn adapter() {}\n").unwrap();
    full_scope.exclude.insert(
        "src/adapter.rs".into(),
        "Thin RPC adapter outside the historic core, still included in the full-source metric"
            .into(),
    );
    full_scope.validate_source_tree(root.path()).unwrap();
    full_scope
        .validate_source_line(root.path(), "src/adapter.rs", 1, &mut lengths)
        .unwrap();
    assert!(
        full_scope
            .validate_source_line(root.path(), "src/adapter.rs", 2, &mut lengths)
            .is_err()
    );
    fs::remove_file(root.path().join("src/main.rs")).unwrap();
    assert!(
        full_scope
            .validate_source_line(root.path(), "src/main.rs", 1, &mut BTreeMap::new())
            .is_err()
    );
}

#[test]
fn coverage_policy_rejects_silent_floor_changes_and_missing_legacy_basis() {
    let mut scope = reviewed_scope();
    assert!(scope.validate().is_ok());
    scope.floor_percent = 84;
    assert!(scope.validate().is_err());
    scope.floor_percent = 85;
    scope.legacy_pyproject_sha256 = "unreviewed".into();
    assert!(scope.validate().is_err());
}

#[test]
fn coverage_policy_rejects_ambiguous_unexplained_or_escaping_paths() {
    let mut scope = reviewed_scope();
    scope
        .exclude
        .insert("src/main.rs".into(), "Contradiction".into());
    assert!(scope.validate().is_err());
    scope.exclude.clear();
    scope
        .include
        .insert("src/../private.rs".into(), "Escape".into());
    assert!(scope.validate().is_err());
    scope.include.remove("src/../private.rs");
    scope.include.insert("src/worker.rs".into(), " ".into());
    assert!(scope.validate().is_err());
}

#[test]
fn coverage_new_source_requires_review_even_before_it_has_executable_lines() {
    let root = tempfile::TempDir::new().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let scope = reviewed_scope();
    scope.validate_source_tree(root.path()).unwrap();
    fs::write(
        root.path().join("src/new.rs"),
        "// intentionally empty module\n",
    )
    .unwrap();
    assert!(scope.validate_source_tree(root.path()).is_err());
    fs::remove_file(root.path().join("src/new.rs")).unwrap();
    fs::remove_file(root.path().join("src/main.rs")).unwrap();
    assert!(scope.validate_source_tree(root.path()).is_err());
}

#[test]
fn coverage_parser_unions_actual_hits_without_counting_dependency_or_test_lines() {
    let report=parse_lcov_report("SF:/work/Telegram-mcp/src/main.rs\nDA:7,0\nDA:7,4\nDA:8,0\nend_of_record\nDA:99,1\nSF:/cargo/registry/vendor/src/lib.rs\nDA:1,90\nend_of_record\nSF:/work/Telegram-mcp/tests/regression.rs\nDA:1,1\nend_of_record\n", "Telegram-mcp").unwrap();
    assert_eq!(
        report,
        BTreeMap::from([
            (("src/main.rs".into(), 7), true),
            (("src/main.rs".into(), 8), false)
        ])
    );
}

#[test]
fn coverage_parser_refuses_corrupt_owned_line_numbers_and_hit_counts() {
    for data in ["0,1", "wrong,1", "1,wrong", "1", ",1", "1,"] {
        assert!(
            parse_lcov_report(
                &format!("SF:src/main.rs\nDA:{data}\nend_of_record\n"),
                "telegram-mcp"
            )
            .is_err()
        );
    }
}
#[test]
fn coverage_normalizes_owned_source_across_operating_systems() {
    assert_eq!(
        owned_source(
            "/home/runner/work/Telegram-mcp/Telegram-mcp/src/state.rs",
            "Telegram-mcp"
        ),
        Some("src/state.rs".into())
    );
    assert_eq!(
        owned_source(
            r"D:\a\Telegram-mcp\Telegram-mcp\src\state.rs",
            "Telegram-mcp"
        ),
        Some("src/state.rs".into())
    );
    assert_eq!(
        owned_source("/work/Telegram-mcp/build.rs", "Telegram-mcp"),
        Some("build.rs".into())
    );
    assert_eq!(
        owned_source("/cargo/registry/vendor-1.0/src/lib.rs", "Telegram-mcp"),
        None
    );
    assert_eq!(
        owned_source("/work/Telegram-mcp/target/generated.rs", "Telegram-mcp"),
        None
    );
}

#[test]
fn repository_coverage_scope_matches_source_tree() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scope: CoverageScope = serde_json::from_slice(
        &fs::read(manifest_dir.join(".github/coverage-scope.json")).unwrap(),
    )
    .unwrap();
    scope.validate().unwrap();
    scope.validate_source_tree(manifest_dir).unwrap();
}
