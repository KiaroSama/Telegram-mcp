use super::*;
#[test]
fn git_revision_rejects_option_injection_and_missing_hash() {
    assert!(scope::revision(Some("--all")).is_err());
    assert!(scope::revision(Some("aabb")).is_err());
    assert!(scope::revision(None).is_err());
    let valid = "ab".repeat(20);
    assert_eq!(scope::revision(Some(&valid)).unwrap(), valid);
}
fn push_repo() -> (tempfile::TempDir, String, String) {
    let repo = tempfile::TempDir::new().unwrap();
    repo_git(repo.path(), &["init", "--quiet", "--object-format=sha1"]).unwrap();
    repo_git(repo.path(), &["commit", "--allow-empty", "-m", "before"]).unwrap();
    let before = repo_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
    repo_git(repo.path(), &["commit", "--allow-empty", "-m", "after"]).unwrap();
    let head = repo_git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
    (repo, before.trim().into(), head.trim().into())
}

fn repo_git(repo: &Path, args: &[&str]) -> Result<String> {
    process::text(
        Command::new("git")
            .current_dir(repo)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .args([
                "-c",
                "user.name=Scope Test",
                "-c",
                "user.email=scope@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.hooksPath=disabled-hooks",
            ])
            .args(args),
        10,
    )
}

fn tag_oid(repo: &Path, name: &str, target: &str) -> String {
    repo_git(repo, &["tag", "-a", name, target, "-m", "test tag"]).unwrap();
    repo_git(repo, &["rev-parse", name]).unwrap().trim().into()
}

#[test]
fn push_scope_accepts_a_commit_at_head() {
    let (repo, _, head) = push_repo();
    scope::validate_push_head(&head, &head, |args| repo_git(repo.path(), args)).unwrap();
}

#[test]
fn push_scope_accepts_an_annotated_tag_at_head() {
    let (repo, _, head) = push_repo();
    let after = tag_oid(repo.path(), "at-head", &head);
    assert_ne!(after, head);
    scope::validate_push_head(&after, &head, |args| repo_git(repo.path(), args)).unwrap();
}

#[test]
fn push_scope_rejects_a_different_commit_or_an_unpeelable_object() {
    let (repo, before, head) = push_repo();
    let tag = tag_oid(repo.path(), "before-head", &before);
    let tree = repo_git(repo.path(), &["rev-parse", "HEAD^{tree}"]).unwrap();
    for after in [before.as_str(), &tag, tree.trim()] {
        let error = scope::validate_push_head(after, &head, |args| repo_git(repo.path(), args))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Push introduction scan checkout differs from the pushed head"
        );
    }
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
