use super::*;

fn package_image(name: &str) -> Vec<u8> {
    package_image_for_target(name, "x86_64-unknown-linux-gnu")
}
fn package_image_for_target(name: &str, target: &str) -> Vec<u8> {
    let mut image = vec![
        0;
        if target.contains("windows") {
            64 + 24 + 240
        } else {
            90
        }
    ];
    let arm = target.starts_with("aarch64-");
    if target.contains("linux") {
        image[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        image[16..18].copy_from_slice(&2u16.to_le_bytes());
        image[18..20].copy_from_slice(&(if arm { 183u16 } else { 62 }).to_le_bytes());
    } else {
        image[..2].copy_from_slice(b"MZ");
        image[60..64].copy_from_slice(&64u32.to_le_bytes());
        image[64..68].copy_from_slice(b"PE\0\0");
        image[68..70].copy_from_slice(&(if arm { 0xaa64u16 } else { 0x8664 }).to_le_bytes());
        image[84..86].copy_from_slice(&240u16.to_le_bytes());
        image[86..88].copy_from_slice(&2u16.to_le_bytes());
        image[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    }
    image.extend_from_slice(name.as_bytes());
    image
}

#[test]
fn case_recorder_records_actual_cpu_and_compiler_host_for_a_native_label() {
    let root = tempfile::tempdir().unwrap();
    let report = root.path().join("junit.xml");
    let output = root.path().join("record.json");
    fs::write(
        &report,
        r#"<testsuite name="native" tests="1"><testcase name="ran"/></testsuite>"#,
    )
    .unwrap();
    let label = format!("{}-{}", env::consts::OS, env::consts::ARCH);
    record(&label, &report, &output).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["architecture"], env::consts::ARCH);
    let compiler = process::text(Command::new("rustc").arg("-vV"), 15).unwrap();
    let target = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap();
    assert_eq!(value["target"], target);
    let other_architecture = if env::consts::ARCH == "aarch64" {
        "x86_64"
    } else {
        "aarch64"
    };
    let wrong_label = format!("{}-{other_architecture}", env::consts::OS);
    let wrong_output = root.path().join("wrong-record.json");
    assert!(
        record(&wrong_label, &report, &wrong_output).is_err(),
        "A current-host execution was accepted under the other architecture's label"
    );
    let retained: serde_json::Value =
        serde_json::from_slice(&fs::read(wrong_output).unwrap()).unwrap();
    assert_eq!(retained["architecture"], env::consts::ARCH);
}

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
fn case_recorder_retains_skipped_evidence_but_refuses_to_report_success() {
    let root = tempfile::tempdir().unwrap();
    let report = root.path().join("junit.xml");
    let destination = root.path().join("record.json");
    fs::write(&report,r#"<testsuite name="native" tests="2"><testcase name="ran"/><testcase name="skipped"><skipped/></testcase></testsuite>"#).unwrap();
    let result = record("synthetic-skipped-regression", &report, &destination);
    let retained: Record = serde_json::from_slice(&fs::read(destination).unwrap()).unwrap();
    assert_eq!(retained.total, 2);
    assert_eq!(retained.skipped, 1);
    assert_eq!(retained.executed, vec!["native::ran"]);
    assert!(
        result.is_err(),
        "A partially skipped run was reported as successful"
    );
    assert!(result.unwrap_err().to_string().contains("skipped"));
    assert!(require_complete_execution(&retained).is_err());
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
    for (_, _, target) in NATIVE_TARGETS {
        let root = tempfile::TempDir::new().unwrap();
        let source = root.path().join("release");
        fs::create_dir(&source).unwrap();
        let suffix = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
            fs::write(
                source.join(format!("{name}{suffix}")),
                package_image_for_target(name, target),
            )
            .unwrap();
        }
        for name in ["LICENSE", "NOTICE", "README.md"] {
            fs::write(root.path().join(name), name).unwrap();
        }
        let output = root.path().join("package");
        package_files(
            &source,
            &output,
            root.path(),
            suffix,
            serde_json::json!({"commit":"synthetic-build","target":target}),
        )
        .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("build.json")).unwrap()).unwrap();
        assert_eq!(metadata["binaries"].as_object().unwrap().len(), 3);
        for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
            assert_eq!(
                metadata["binaries"][name]["sha256"],
                process::digest(&output.join(format!("{name}{suffix}"))).unwrap()
            );
        }
    }
}
#[test]
fn release_package_refuses_missing_helper_before_creating_output() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("release");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("telegram-mcp.exe"),
        package_image_for_target("server", "x86_64-pc-windows-msvc"),
    )
    .unwrap();
    let output = root.path().join("package");
    let error = package_files(
        &source,
        &output,
        root.path(),
        ".exe",
        serde_json::json!({"target":"x86_64-pc-windows-msvc"}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "A required native release executable is missing"
    );
    assert!(!output.exists());
}
#[test]
fn release_package_cannot_upload_stale_or_owner_files() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("release");
    fs::create_dir(&source).unwrap();
    for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
        fs::write(source.join(name), package_image(name)).unwrap();
    }
    for name in ["LICENSE", "NOTICE", "README.md"] {
        fs::write(root.path().join(name), name).unwrap();
    }
    package_files(
        &source,
        &root.path().join("valid-control"),
        root.path(),
        "",
        serde_json::json!({"target":"x86_64-unknown-linux-gnu"}),
    )
    .unwrap();
    let output = root.path().join("package");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("owner.txt"), "preserve").unwrap();
    assert!(
        package_files(
            &source,
            &output,
            root.path(),
            "",
            serde_json::json!({"target":"x86_64-unknown-linux-gnu"})
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(output.join("owner.txt")).unwrap(),
        "preserve"
    );
}

#[test]
fn release_package_refuses_an_image_with_another_native_architecture() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("release");
    fs::create_dir(&source).unwrap();
    for name in ["telegram-mcp", "emoji-studio", "operator-plugin"] {
        fs::write(source.join(name), package_image(name)).unwrap();
    }
    for name in ["LICENSE", "NOTICE", "README.md"] {
        fs::write(root.path().join(name), name).unwrap();
    }
    let output = root.path().join("refused");
    assert!(
        package_files(
            &source,
            &output,
            root.path(),
            "",
            serde_json::json!({"target":"aarch64-unknown-linux-gnu"})
        )
        .is_err()
    );
    assert!(!output.exists());
}

fn case_fixture(label: &str, leg: &NativeLeg) -> Record {
    Record {
        schema_version: 2,
        label: label.into(),
        platform: leg.platform.clone(),
        architecture: leg.architecture.clone(),
        target: leg.target.clone(),
        rustc: "rustc 1.100.0 (synthetic fixture)".into(),
        commit: "ab".repeat(20),
        lock_sha256: "cd".repeat(32),
        total: 1,
        failed: 0,
        skipped: 0,
        cases: vec!["native::ran".into()],
        executed: vec!["native::ran".into()],
    }
}

#[test]
fn both_architectures_require_complete_native_and_renderer_identity_evidence() {
    let expected = native_legs().unwrap();
    let records: Vec<_> = expected
        .iter()
        .map(|(label, leg)| case_fixture(label, leg))
        .collect();
    let cases = BTreeMap::from([
        ("linux".into(), vec!["native::ran".into()]),
        ("windows".into(), vec!["native::ran".into()]),
    ]);
    let commit = "ab".repeat(20);
    let lock = "cd".repeat(32);
    validate_record_set(&records, &expected, &cases, &commit, &lock).unwrap();
    for field in [
        "architecture",
        "target",
        "platform",
        "schema_version",
        "rustc",
    ] {
        let mut changed = records.clone();
        match field {
            "architecture" => changed[0].architecture = "x86".into(),
            "target" => changed[0].target = "aarch64-pc-windows-msvc".into(),
            "platform" => changed[0].platform = "unobserved".into(),
            "schema_version" => changed[0].schema_version = 1,
            _ => changed[0].rustc = "rustc 1.99.0".into(),
        }
        assert!(
            validate_record_set(&changed, &expected, &cases, &commit, &lock).is_err(),
            "Mislabeled {field} was accepted"
        );
    }
    let mut missing = records.clone();
    missing.pop();
    assert!(validate_record_set(&missing, &expected, &cases, &commit, &lock).is_err());
    let mut duplicate = records.clone();
    duplicate.push(records[0].clone());
    assert!(validate_record_set(&duplicate, &expected, &cases, &commit, &lock).is_err());
    let mut mismatched_runner = records.clone();
    let arm = mismatched_runner
        .iter_mut()
        .find(|r| r.label == "linux-aarch64")
        .unwrap();
    arm.architecture = "x86_64".into();
    arm.target = "x86_64-unknown-linux-gnu".into();
    assert!(validate_record_set(&mismatched_runner, &expected, &cases, &commit, &lock).is_err());
}

#[test]
fn architecture_manifest_and_record_fields_cannot_silently_omit_a_target() {
    let expected = native_legs().unwrap();
    let mut missing = expected.clone();
    missing.remove("windows-aarch64");
    assert!(validate_leg_manifest(&missing).is_err());
    let mut mislabeled = expected.clone();
    mislabeled.get_mut("linux-x86_64").unwrap().target = "aarch64-unknown-linux-gnu".into();
    assert!(validate_leg_manifest(&mislabeled).is_err());
    let (label, leg) = expected.iter().next().unwrap();
    let value = serde_json::to_value(case_fixture(label, leg)).unwrap();
    for field in ["architecture", "target"] {
        let mut absent = value.clone();
        absent.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Record>(absent).is_err());
    }
}

#[test]
fn parity_rows_rejects_missing_duplicate_extra_wrong_sha_and_wrong_area() {
    use crate::parity_rows::{InventoryFile, InventoryItem, ParityRow, Verdict, reconcile_rows};

    let sha = "a".repeat(40);
    let areas: BTreeSet<String> = ["A1_area".into(), "B1_area".into()].into_iter().collect();
    let inv = InventoryFile {
        items: vec![
            InventoryItem {
                file: "pkg/a.py".into(),
                qual: "alpha".into(),
                kind: "function".into(),
                line: 10,
                area: "A1_area".into(),
                status: "PARTIAL".into(),
            },
            InventoryItem {
                file: "pkg/b.py".into(),
                qual: "beta".into(),
                kind: "function".into(),
                line: 20,
                area: "B1_area".into(),
                status: "MISSING".into(),
            },
        ],
    };
    let make_row = |file: &str, qual: &str, line: u64, area: &str| ParityRow {
        path: file.into(),
        qualified_symbol: qual.into(),
        kind: "function".into(),
        line,
        area: area.into(),
        candidate_sha: sha.clone(),
        owning_track: "T1".into(),
        mapping: "src/a.rs::alpha".into(),
        method: "unit_fixture".into(),
        evidence: "tests/unit/a.rs::alpha_ok".into(),
        verdict: Verdict::Matched,
        authorization_ref: None,
    };

    let valid = vec![
        make_row("pkg/a.py", "alpha", 10, "A1_area"),
        make_row("pkg/b.py", "beta", 20, "B1_area"),
    ];
    let (sorted, summary) = reconcile_rows(&inv, &valid, &sha, &areas).unwrap();
    assert_eq!(sorted.len(), 2);
    assert_eq!(summary.verdicts.matched, 2);

    // Missing key
    assert!(reconcile_rows(&inv, &valid[..1], &sha, &areas).is_err());

    // Duplicate key
    let mut dup = valid.clone();
    dup.push(valid[0].clone());
    assert!(reconcile_rows(&inv, &dup, &sha, &areas).is_err());

    // Extra key
    let mut extra = valid.clone();
    extra.push(make_row("pkg/c.py", "gamma", 30, "A1_area"));
    assert!(reconcile_rows(&inv, &extra, &sha, &areas).is_err());

    // Wrong SHA
    let mut bad_sha = valid.clone();
    bad_sha[0].candidate_sha = "b".repeat(40);
    assert!(reconcile_rows(&inv, &bad_sha, &sha, &areas).is_err());

    // Wrong area
    let mut bad_area = valid.clone();
    bad_area[0].area = "B1_area".into();
    assert!(reconcile_rows(&inv, &bad_area, &sha, &areas).is_err());
}

#[test]
fn parity_rows_rejects_unsupported_bridge_absent_evidence_and_miscounted_authorized_diffs() {
    use crate::parity_rows::{InventoryFile, InventoryItem, ParityRow, Verdict, reconcile_rows};

    let sha = "c".repeat(40);
    let areas: BTreeSet<String> = ["C1_area".into()].into_iter().collect();
    let inv = InventoryFile {
        items: vec![InventoryItem {
            file: "pkg/c.py".into(),
            qual: "check".into(),
            kind: "function".into(),
            line: 5,
            area: "C1_area".into(),
            status: "PARTIAL".into(),
        }],
    };
    let base = ParityRow {
        path: "pkg/c.py".into(),
        qualified_symbol: "check".into(),
        kind: "function".into(),
        line: 5,
        area: "C1_area".into(),
        candidate_sha: sha.clone(),
        owning_track: "T3".into(),
        mapping: "src/c.rs::check".into(),
        method: "rust_anything_gate".into(),
        evidence: "gate:ok".into(),
        verdict: Verdict::Matched,
        authorization_ref: None,
    };

    // Unsupported bridge claimed as matched -> error
    let mut unsupported = base.clone();
    unsupported.method = "unsupported_bridge".into();
    assert!(reconcile_rows(&inv, &[unsupported], &sha, &areas).is_err());

    // Complete set of names with absent behavioral evidence -> error
    let mut no_evidence = base.clone();
    no_evidence.evidence.clear();
    assert!(reconcile_rows(&inv, &[no_evidence], &sha, &areas).is_err());

    // Authorized difference counted as match -> error
    let mut miscounted_auth = base.clone();
    miscounted_auth.authorization_ref = Some("spec.md#FR-010".into());
    assert!(reconcile_rows(&inv, &[miscounted_auth], &sha, &areas).is_err());

    // Properly classified explicitly_authorized_difference does NOT count as matched
    let mut auth = base;
    auth.verdict = Verdict::ExplicitlyAuthorizedDifference;
    auth.authorization_ref = Some("spec.md#FR-010".into());
    let (_, summary) = reconcile_rows(&inv, &[auth], &sha, &areas).unwrap();
    assert_eq!(summary.verdicts.matched, 0);
    assert_eq!(summary.verdicts.verified_native_equivalent, 0);
    assert_eq!(summary.verdicts.explicitly_authorized_difference, 1);
}

#[test]
fn parity_rows_detects_nondeterministic_replays() {
    use crate::parity_rows::{ParityRow, Verdict, compare_replays};

    let sha = "d".repeat(40);
    let r1 = ParityRow {
        path: "pkg/d.py".into(),
        qualified_symbol: "delta".into(),
        kind: "function".into(),
        line: 1,
        area: "D_core".into(),
        candidate_sha: sha,
        owning_track: "T3".into(),
        mapping: "src/d.rs::delta".into(),
        method: "unit_fixture".into(),
        evidence: "tests/unit/d.rs::ok".into(),
        verdict: Verdict::Matched,
        authorization_ref: None,
    };
    let mut r2 = r1.clone();
    assert!(compare_replays(std::slice::from_ref(&r1), std::slice::from_ref(&r2)).is_ok());

    r2.verdict = Verdict::Unresolved;
    assert!(compare_replays(&[r1], &[r2]).is_err());
}

#[test]
fn parity_rows_reconcile_files_and_verify_replay_files_round_trip() {
    use crate::parity_rows::{reconcile_files, verify_replay_files};

    let temp = tempfile::tempdir().unwrap();
    let sha = "e".repeat(40);
    let inv_path = temp.path().join("inv.json");
    let ev1_path = temp.path().join("ev1.jsonl");
    let ev2_path = temp.path().join("ev2.jsonl");
    let out1 = temp.path().join("out/candidate.jsonl");
    let out2 = temp.path().join("out/replay.jsonl");
    let bad_jsonl = temp.path().join("bad.jsonl");

    std::fs::write(
        &inv_path,
        serde_json::to_vec(&serde_json::json!({
            "items": [
                {"file": "a.py", "qual": "fn_a", "kind": "function", "line": 1, "area": "A1", "status": "MATCHED"},
                {"file": "b.py", "qual": "fn_b", "kind": "function", "line": 2, "area": "B1", "status": "PARTIAL"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let row_a = serde_json::json!({
        "path": "a.py", "qualified_symbol": "fn_a", "kind": "function", "line": 1,
        "area": "A1", "candidate_sha": sha, "owning_track": "T1",
        "mapping": "src/a.rs::fn_a", "method": "unit_fixture", "evidence": "tests/a.rs", "verdict": "matched"
    });
    let row_b = serde_json::json!({
        "path": "b.py", "qualified_symbol": "fn_b", "kind": "function", "line": 2,
        "area": "B1", "candidate_sha": sha, "owning_track": "T2",
        "mapping": "src/b.rs::fn_b", "method": "native_equiv", "evidence": "tests/b.rs", "verdict": "verified_native_equivalent"
    });
    std::fs::write(&ev1_path, format!("{row_a}\n\n{row_b}\n")).unwrap();
    std::fs::write(&ev2_path, format!("{row_b}\n{row_a}\n")).unwrap();
    std::fs::write(&bad_jsonl, "{not json}\n").unwrap();

    let s1 = reconcile_files(&inv_path, &ev1_path, &out1, &sha).unwrap();
    let s2 = reconcile_files(&inv_path, &ev2_path, &out2, &sha).unwrap();
    assert_eq!(s1, s2);
    assert_eq!(verify_replay_files(&out1, &out2).unwrap(), 2);
    assert!(reconcile_files(&inv_path, &bad_jsonl, &out1, &sha).is_err());
}
