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

#[test]
fn takeover_recognizes_global_config_and_preserves_unrelated_processes() {
    let checkout = Checkout::new();
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("start-mcp.ps1");
    let mut command = checkout.command("pwsh");
    success(run(command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
        $ErrorActionPreference = 'Stop'
        function Write-FixtureLog {
            param([ValidateSet('INFO', 'WARNING', 'ERROR', 'DEBUG')][string]$Level, [string]$Message)
            [Console]::Error.WriteLine("[$([DateTime]::UtcNow.ToString('o'))] [$Level] $Message")
        }
        $tokens = $null; $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($env:FIXTURE_LAUNCHER, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw 'Launcher did not parse' }
        $fn = $ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Stop-OtherInstances'}, $false)
        if (-not $fn) { throw 'Takeover function is missing' }
        . ([ScriptBlock]::Create($fn.Extent.Text))
        # Use a controlled Windows process inventory on every host; never stop real processes.
        Set-Variable -Name IsWindows -Value $true -Force
        $root = 'C:\synthetic checkout'
        $binary = "$root\bin\telegram-mcp.exe"
        $launcher = "$root\start-mcp.ps1"
        $script:Killed = [Collections.Generic.List[int]]::new()
        $script:Inventory = @(
            [pscustomobject]@{ ProcessId = 830001; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" --config `"C:\private config\account.env`" serve --http 127.0.0.1:8765" },
            [pscustomobject]@{ ProcessId = 830002; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" --config=`"C:\private config\account.env`" serve" },
            [pscustomobject]@{ ProcessId = 830003; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" `"--config=C:\private config\account.env`" serve" },
            [pscustomobject]@{ ProcessId = 830004; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" serve --config account.env" },
            [pscustomobject]@{ ProcessId = 830005; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" --config `"C:\private config\serve.env`" manage" },
            [pscustomobject]@{ ProcessId = 830006; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = '"C:\another checkout\bin\telegram-mcp.exe" serve' },
            [pscustomobject]@{ ProcessId = 830007; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"C:\another checkout\telegram-mcp.exe`" --config `"$binary serve configuration.env`" manage" },
            [pscustomobject]@{ ProcessId = 830008; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" --config `"C:\private serve configuration\account.env`" manage" },
            [pscustomobject]@{ ProcessId = 830009; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" --config=`"C:\private serve configuration\account.env`" manage" },
            [pscustomobject]@{ ProcessId = 830010; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "`"C:\Program Files\PowerShell\7\pwsh.exe`" -NoLogo -NoProfile -NonInteractive -File `"$launcher`" --config account.env" },
            [pscustomobject]@{ ProcessId = 830011; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -NoProfile -File `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830012; ParentProcessId = 0; Name = 'powershell.exe'; CommandLine = "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830013; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -nop -noni -ep Bypass -w Hidden -f `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830014; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "C:\tools\pwsh.exe -WorkingDirectory `"C:\another directory`" -File `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830020; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -File C:\another\worker.ps1 -Config `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830021; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -Command Write-Output `"$launcher`"; Start-Sleep 120" },
            [pscustomobject]@{ ProcessId = 830022; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -NoProfile C:\another\worker.ps1 -File `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830023; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -CommandWithArgs Write-Output -File `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830024; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -File $launcher" },
            [pscustomobject]@{ ProcessId = 830025; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -Command & `"$launcher`"" },
            [pscustomobject]@{ ProcessId = 830026; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = "pwsh.exe -WorkingDirectory `"$launcher`" -File C:\another\worker.ps1" },
            [pscustomobject]@{ ProcessId = $PID; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = "`"$binary`" serve" }
        )
        function Get-CimInstance { param($ClassName, $ErrorAction) $script:Inventory }
        function Get-Process { param($Id, $ErrorAction) }
        function taskkill.exe {
            $script:Killed.Add([int]$args[1])
        }
        $result = Stop-OtherInstances -Root $root -WaitSeconds 0
        $expected = 830001, 830002, 830003, 830004, 830010, 830011, 830012, 830013, 830014
        if (($result.Stopped | Sort-Object) -join ',' -ne ($expected -join ',') -or $result.Survived.Count) {
            Write-FixtureLog ERROR 'Takeover did not select precisely this checkout''s native servers and explicit launcher invocations.'
            throw "Unexpected selected process identities: $($result.Stopped -join ',')"
        }
        if (($script:Killed | Sort-Object) -join ',' -ne ($expected -join ',')) { throw 'Takeover did not stop exactly the selected process trees' }
        $script:Killed.Clear()
        $script:Inventory = @(
            [pscustomobject]@{ ProcessId = 830030; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = 'pwsh.exe -NoProfile -File C:\checkout\start-mcp.ps1' },
            [pscustomobject]@{ ProcessId = 830031; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = 'pwsh.exe -File C:\another\worker.ps1 C:\checkout\start-mcp.ps1' }
        )
        $unquoted = Stop-OtherInstances -Root 'C:\checkout' -WaitSeconds 0
        if ($unquoted.Stopped.Count -ne 1 -or $unquoted.Stopped[0] -ne 830030 -or $unquoted.Survived.Count -or $script:Killed.Count -ne 1 -or $script:Killed[0] -ne 830030) {
            throw 'Takeover did not distinguish an unquoted launcher path from another script''s arguments'
        }
        Write-FixtureLog INFO 'Native configuration and explicit launcher forms selected; unrelated script arguments, command text, and current process preserved.'
    "#]).env("FIXTURE_LAUNCHER", shipped), ""));
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
