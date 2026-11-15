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

#[cfg(target_os = "linux")]
#[test]
fn scanner_archive_larger_than_diagnostic_budget_extracts_exactly_without_capture() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    let member = source.join("gitleaks");
    fs::File::create(&member)
        .unwrap()
        .set_len(20 * 1024 * 1024)
        .unwrap();
    let archive = root.path().join("scanner.tar.gz");
    process::text(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("gitleaks"),
        15,
    )
    .unwrap();
    let extracted = unpack_verified_scanner(&archive, &destination).unwrap();
    assert_eq!(fs::metadata(&extracted).unwrap().len(), 20 * 1024 * 1024);
    assert_eq!(
        process::digest(&extracted).unwrap(),
        process::digest(&member).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn scanner_archive_refuses_a_symlink_before_touching_its_target() {
    let root = tempfile::TempDir::new().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    let owner = root.path().join("owner.bin");
    fs::write(&owner, b"preserve owner bytes").unwrap();
    std::os::unix::fs::symlink(&owner, source.join("gitleaks")).unwrap();
    let archive = root.path().join("scanner.tar.gz");
    process::text(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("gitleaks"),
        15,
    )
    .unwrap();
    assert!(unpack_verified_scanner(&archive, &destination).is_err());
    assert!(!destination.join("gitleaks").exists());
    assert_eq!(fs::read(owner).unwrap(), b"preserve owner bytes");
}
