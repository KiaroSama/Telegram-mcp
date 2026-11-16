//! End-to-end native helper checks: explicit source, destination, exit and private log.
use std::{fs, path::Path, process::Command};

fn temporary_root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_operator-plugin"));
    let files = root.join("files");
    fs::create_dir_all(&files).unwrap();
    command
        .current_dir(root)
        .env("TELEGRAM_STATE_DIR", root.join("private-state"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&[files]).unwrap(),
        )
        .arg("--source")
        .arg(env!("CARGO_MANIFEST_DIR"));
    command
}

#[test]
fn native_installer_cli_uses_explicit_source_and_preserves_collision() {
    let root = temporary_root();
    let project = root.path().join("target workspace");
    let run = command(root.path())
        .args(["install", "--client", "opencode", "--project"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8(run.stdout)
            .unwrap()
            .contains("Installed five complete skills")
    );
    assert_eq!(
        fs::read_dir(project.join(".opencode/skills"))
            .unwrap()
            .count(),
        5
    );
    let again = command(root.path())
        .args(["install", "--client", "opencode", "--project"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(
        String::from_utf8(again.stderr)
            .unwrap()
            .contains("not overwritten")
    );
    assert_eq!(
        fs::read_dir(project.join(".opencode/skills"))
            .unwrap()
            .count(),
        5
    );
    let log = fs::read_to_string(root.path().join("private-state/mcp.log")).unwrap();
    assert!(log.contains("Operator guidance operation completed"));
    assert!(log.contains("Operator helper refused or failed"));
}

#[test]
fn native_packager_cli_creates_a_real_archive_without_runtime_state() {
    let root = temporary_root();
    let output = root.path().join("operator.zip");
    let run = command(root.path())
        .args(["package", "--client", "portable", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let archive = zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap();
    assert_eq!(archive.len(), 18);
    for name in archive.file_names() {
        let name = name.unwrap();
        assert!(
            !name.ends_with(".env")
                && !name.contains("private-state")
                && !name.ends_with(".session")
        );
    }
}

#[test]
fn native_installer_cli_requires_an_explicit_destination() {
    let root = temporary_root();
    let run = command(root.path()).arg("install").output().unwrap();
    assert!(!run.status.success());
    assert!(
        String::from_utf8(run.stderr)
            .unwrap()
            .contains("Choose --project")
    );
    assert!(!root.path().join(".agents").exists());
}
