use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const NATIVE: &str = env!("CARGO_BIN_EXE_telegram-mcp");

#[path = "rust_launchers/accounts.rs"]
mod accounts;
#[cfg(windows)]
#[path = "rust_launchers/windows.rs"]
mod windows;

struct Checkout {
    dir: tempfile::TempDir,
}

impl Checkout {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix("launcher checkout ")
                .tempdir()
                .unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn env_file(&self) -> PathBuf {
        self.root().join(".env")
    }

    fn state(&self) -> PathBuf {
        self.root().join("local state")
    }

    fn files(&self, parent: &str, prefix: &str) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.root().join(parent)) else {
            return Vec::new();
        };
        let mut matches: Vec<_> = entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(prefix)
            })
            .collect();
        matches.sort();
        matches
    }

    fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if name.starts_with("TELEGRAM_") || name.starts_with("MCP_") {
                command.env_remove(key);
            }
        }
        command
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.root().join("xdg state"))
            .env("NO_COLOR", "1");
        command
    }

    fn native(&self, args: &[&str], input: &str) -> Output {
        run(self.command(NATIVE).args(args), input)
    }

    fn launcher_command(&self, script: &str) -> Command {
        let path = self.root().join(script);
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join(script), &path).unwrap();
        let mut command = self.command("pwsh");
        // Exercise script-relative discovery from outside the fixture checkout.
        command.current_dir(self.root().parent().unwrap()).args([
            std::ffi::OsStr::new("-NoProfile"),
            std::ffi::OsStr::new("-NonInteractive"),
            std::ffi::OsStr::new("-File"),
            path.as_os_str(),
        ]);
        command
    }
}

// File-backed captures drain both streams without pipe backpressure while polling the child.
fn run(command: &mut Command, input: &str) -> Output {
    let capture = tempfile::tempdir().unwrap();
    let stdout = capture.path().join("stdout");
    let stderr = capture.path().join("stderr");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(File::create(&stdout).unwrap())
        .stderr(File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    if let Some(mut pipe) = child.stdin.take() {
        match pipe.write_all(input.as_bytes()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(error) => {
                kill_tree(&mut child);
                panic!("could not write synthetic input: {error}");
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            result => {
                kill_tree(&mut child);
                panic!("fixture did not exit before its deadline: {result:?}");
            }
        }
    };
    let read = |path: &Path| {
        let mut bytes = Vec::new();
        File::open(path)
            .unwrap()
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    };
    Output {
        status,
        stdout: read(&stdout),
        stderr: read(&stderr),
    }
}

fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn portable_start_launcher_preserves_protocol_diagnostics_and_exit() {
    let checkout = Checkout::new();
    let fixture = checkout.root().join("native fixture.ps1");
    fs::write(
        &fixture,
        r#"
        if ($args[0] -ne 'serve' -or $env:TELEGRAM_MCP_LOG_FILE -ne 'off') { exit 91 }
        [Console]::Out.WriteLine('synthetic protocol frame')
        [Console]::Error.WriteLine('synthetic diagnostic')
        exit 23
    "#,
    )
    .unwrap();
    let output = run(
        checkout
            .launcher_command("start-mcp.ps1")
            .env("TELEGRAM_MCP_BINARY", &fixture)
            .arg("-NoLogToFile"),
        "",
    );
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "synthetic protocol frame"
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("synthetic diagnostic")
    );
    assert!(!checkout.root().join("xdg state").exists());
}

fn success(output: Output) -> (String, String) {
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}: {stderr}\n{stdout}",
        output.status
    );
    (stdout, stderr)
}
