use super::*;

fn launch(checkout: &Checkout, script: &str, args: &[&str], input: &str) -> Output {
    run(
        checkout
            .launcher_command(script)
            .env("TELEGRAM_MCP_BINARY", NATIVE)
            .args(args),
        input,
    )
}

#[test]
fn manager_launcher_executes_native_menu_with_relative_state_and_utf8() {
    let checkout = Checkout::new();
    let config = checkout.root().join("config with spaces.env");
    fs::write(&config, "TELEGRAM_SESSION_NAME_WORK=native\n").unwrap();
    fs::write(checkout.env_file(), "TELEGRAM_STATE_DIR='local state'\n").unwrap();
    let (text, _) = success(launch(
        &checkout,
        "Manage-Accounts.ps1",
        &["--config", "config with spaces.env"],
        "1\nexit\n",
    ));
    assert!(text.contains("Telegram MCP account manager"));
    assert!(text.contains("1. work"));
    assert!(checkout.state().is_dir());
    let files = checkout.files("logs", "Manage-Accounts_");
    assert_eq!(files.len(), 1);
    let text = fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("UTC] [INFO] [accounts]"));
    assert!(text.contains("exit code 0"));
    assert!(!text.contains("=native"));
    assert_private(&files[0]);
}

#[test]
fn server_launcher_uses_dotenv_and_keeps_protocol_out_of_private_log() {
    let checkout = Checkout::new();
    fs::write(checkout.env_file(), "TELEGRAM_STATE_DIR=\"local state\"\n").unwrap();
    let (stdout, stderr) = success(launch(&checkout, "start-mcp.ps1", &["--help"], ""));
    assert!(stdout.contains("Usage: telegram-mcp.exe serve"));
    let logs = checkout.files("local state/logs", "start-mcp_");
    assert_eq!(logs.len(), 1);
    let text = fs::read_to_string(&logs[0]).unwrap();
    assert!(text.contains("Starting: telegram-mcp serve"));
    assert!(text.contains("Stopped with exit code 0"));
    assert!(!text.contains("Usage:"));
    assert!(stderr.contains(&logs[0].file_name().unwrap().to_string_lossy().into_owned()));
    assert_private(&logs[0]);
}

#[test]
fn process_state_override_wins_and_both_logging_opt_outs_write_nothing() {
    for use_flag in [true, false] {
        let checkout = Checkout::new();
        fs::write(checkout.env_file(), "TELEGRAM_STATE_DIR='ignored state'\n").unwrap();
        let mut command = checkout.launcher_command("start-mcp.ps1");
        command
            .env("TELEGRAM_MCP_BINARY", NATIVE)
            .env("TELEGRAM_STATE_DIR", checkout.state());
        if use_flag {
            command.arg("-NoLogToFile");
        } else {
            command.env("TELEGRAM_MCP_LAUNCHER_LOG", "off");
        }
        let (stdout, stderr) = success(run(command.arg("--help"), ""));
        assert!(stdout.contains("Usage: telegram-mcp.exe serve"));
        assert!(stderr.contains("disabled by"));
        assert!(checkout.files("local state/logs", "start-mcp_").is_empty());
        assert!(!checkout.root().join("ignored state").exists());
    }
}

#[test]
fn both_launchers_resolve_release_and_bin_and_refuse_invalid_override() {
    let checkout = Checkout::new();
    fs::write(checkout.env_file(), "TELEGRAM_STATE_DIR='local state'\n").unwrap();
    let filename = "telegram-mcp.exe";
    let release = checkout.root().join("target/release");
    fs::create_dir_all(&release).unwrap();
    fs::hard_link(NATIVE, release.join(filename))
        .or_else(|_| fs::copy(NATIVE, release.join(filename)).map(|_| ()))
        .unwrap();
    for (script, usage, flags) in [
        (
            "Manage-Accounts.ps1",
            "Usage: telegram-mcp.exe manage",
            &[][..],
        ),
        (
            "start-mcp.ps1",
            "Usage: telegram-mcp.exe serve",
            &["-NoLogToFile"][..],
        ),
    ] {
        let (stdout, _) = success(run(
            checkout.launcher_command(script).args(flags).arg("--help"),
            "",
        ));
        assert!(stdout.contains(usage));
        let output = run(
            checkout
                .launcher_command(script)
                .args(flags)
                .env("TELEGRAM_MCP_BINARY", checkout.root().join("missing.exe")),
            "",
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("TELEGRAM_MCP_BINARY")
        );
    }
    let bin = checkout.root().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::rename(release.join(filename), bin.join(filename)).unwrap();
    fs::write(release.join(filename), b"must not launch").unwrap();
    for script in ["Manage-Accounts.ps1", "start-mcp.ps1"] {
        success(run(checkout.launcher_command(script).arg("--help"), ""));
    }
}

#[test]
fn start_launcher_forwards_native_failure() {
    let checkout = Checkout::new();
    let output = launch(
        &checkout,
        "start-mcp.ps1",
        &["-NoLogToFile", "--not-a-server-option"],
        "",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--not-a-server-option")
    );
    assert!(
        checkout
            .files("xdg state/telegram-mcp/logs", "start-mcp_")
            .is_empty()
    );
}

#[test]
fn generated_environment_and_backups_have_a_protected_owner_only_acl() {
    let checkout = Checkout::new();
    success(checkout.native(&["manage"], "\nexit\n"));
    assert_private(&checkout.env_file());
    fs::write(
        checkout.env_file(),
        "TELEGRAM_STATE_DIR='local state'\nTELEGRAM_SESSION_NAME_WORK=native\n",
    )
    .unwrap();
    let planted = run(
        checkout
            .command("icacls.exe")
            .arg(checkout.env_file())
            .args(["/grant", "*S-1-1-0:(R)"]),
        "",
    );
    assert!(planted.status.success());
    success(checkout.native(&["manage"], "4\n1\nsecond\n\nexit\n"));
    let backups = checkout.files(".", ".env.backup-");
    assert_eq!(backups.len(), 1);
    assert_private(&checkout.env_file());
    assert_private(&backups[0]);
}

#[test]
fn launcher_logs_prune_oldest_main_and_rotated_runs() {
    let checkout = Checkout::new();
    fs::write(checkout.env_file(), "TELEGRAM_STATE_DIR='local state'\n").unwrap();
    let logs = checkout.state().join("logs");
    fs::create_dir_all(&logs).unwrap();
    for day in 1..=15 {
        let name = format!("start-mcp_2000-01-{day:02}_00-00-00_UTC.log");
        fs::write(logs.join(&name), "old fixture").unwrap();
        fs::write(logs.join(format!("{name}.1")), "rotated fixture").unwrap();
    }
    success(launch(&checkout, "start-mcp.ps1", &["--help"], ""));
    let files = checkout.files("local state/logs", "start-mcp_");
    let count = |suffix: &str| {
        files
            .iter()
            .filter(|path| path.to_string_lossy().ends_with(suffix))
            .count()
    };
    assert_eq!(count(".log"), 10);
    assert_eq!(count(".log.1"), 10);
    assert!(!logs.join("start-mcp_2000-01-06_00-00-00_UTC.log").exists());
    assert!(
        logs.join("start-mcp_2000-01-06_00-00-00_UTC.log.1")
            .exists()
    );
}

#[test]
fn manager_restarts_only_its_checkout_after_success_and_failed_rename() {
    for occupied in [false, true] {
        let checkout = Checkout::new();
        let other = Checkout::new();
        for fixture in [&checkout, &other] {
            fs::write(
                fixture.env_file(),
                "TELEGRAM_STATE_DIR='local state'\nTELEGRAM_SESSION_NAME_WORK=native\n",
            )
            .unwrap();
            fs::write(
                fixture.root().join("start-mcp.ps1"),
                r#"
                $ErrorActionPreference = 'Stop'
                if ($args.Count -ne 0) {
                    if ($args[0] -ne '--config' -or $args[2] -ne '--http') { exit 90 }
                    [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'restarted'), $args[3])
                    exit 0
                }
                [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'ready'), 'ready')
                Start-Sleep -Seconds 120
            "#,
            )
            .unwrap();
        }
        if occupied {
            let path = checkout.state().join("secret-chats");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("work-history.json"), b"source").unwrap();
            fs::write(path.join("second-history.json"), b"destination").unwrap();
        }
        let start = |fixture: &Checkout| {
            FixtureProcess(
                fixture
                    .command("pwsh.exe")
                    .args(["-NoProfile", "-NonInteractive", "-File"])
                    .arg(fixture.root().join("start-mcp.ps1"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        };
        let mut own = start(&checkout);
        let mut foreign = start(&other);
        wait_for_file(&checkout.root().join("ready"));
        wait_for_file(&other.root().join("ready"));
        let (text, _) = success(checkout.native(&["manage"], "4\n1\nsecond\n\nexit\n"));
        assert!(text.contains(if occupied {
            "destination account already has stored files"
        } else {
            "Renamed 'work' to 'second'"
        }));
        assert!(own.0.try_wait().unwrap().is_some());
        assert!(foreign.0.try_wait().unwrap().is_none());
        wait_for_file(&checkout.root().join("restarted"));
        assert!(!other.root().join("restarted").exists());
    }
}

#[test]
fn shipped_takeover_distinguishes_manual_and_automatic_starts() {
    let checkout = Checkout::new();
    let other = Checkout::new();
    let start = |fixture: &Checkout| {
        let script = fixture.root().join("start-mcp.ps1");
        fs::write(
            &script,
            "Set-Content -LiteralPath (Join-Path $PSScriptRoot 'ready') -Value 'ok'\nStart-Sleep -Seconds 120\n",
        )
        .unwrap();
        FixtureProcess(
            fixture
                .command("pwsh.exe")
                .args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut own = start(&checkout);
    let mut foreign = start(&other);
    wait_for_file(&checkout.root().join("ready"));
    wait_for_file(&other.root().join("ready"));
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("start-mcp.ps1");
    let mut command = checkout.command("pwsh.exe");
    success(run(command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
        $ErrorActionPreference = 'Stop'
        $tokens = $null; $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($env:FIXTURE_LAUNCHER, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw 'Launcher did not parse' }
        foreach ($name in 'Test-OpenedByPerson', 'Stop-OtherInstances') {
            $fn = $ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name}, $false)
            if (-not $fn) { throw "Missing function: $name" }
            . ([ScriptBlock]::Create($fn.Extent.Text))
        }
        if (-not (Test-OpenedByPerson -InputRedirected $false -HostArguments @('pwsh', '-File', 'x'))) { throw 'Manual launch did not qualify for takeover' }
        if (Test-OpenedByPerson -InputRedirected $true -HostArguments @('pwsh', '-File', 'x')) { throw 'Redirected launch qualified for takeover' }
        if (Test-OpenedByPerson -InputRedirected $false -HostArguments @('pwsh', '-NonInteractive', '-File', 'x')) { throw 'Supervisor launch qualified for takeover' }
        $result = Stop-OtherInstances -Root $env:FIXTURE_ROOT
        if ($result.Survived.Count -or $result.Stopped -notcontains [int]$env:FIXTURE_PID) { throw 'Checkout process did not stop' }
    "#]).env("FIXTURE_LAUNCHER", shipped).env("FIXTURE_ROOT", checkout.root()).env("FIXTURE_PID", own.0.id().to_string()), ""));
    assert!(own.0.try_wait().unwrap().is_some());
    assert!(foreign.0.try_wait().unwrap().is_none());
}

struct FixtureProcess(Child);

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            kill_tree(&mut self.0);
        }
    }
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "fixture did not create {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn assert_private(path: &Path) {
    let mut command = Command::new("pwsh.exe");
    let (text, _) = success(run(command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
        $ErrorActionPreference = 'Stop'
        $acl = Get-Acl -LiteralPath $env:FIXTURE_ACL_PATH
        if (-not $acl.AreAccessRulesProtected) { throw 'Inherited access is enabled' }
        $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
        $me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $rights = @($rules | ForEach-Object { $_.IdentityReference.Value } | Sort-Object)
        $approved = ($rights.Count -eq 1 -and $rights[0] -eq $me) -or
            (($rights -join ',') -eq 'S-1-3-4,S-1-5-18')
        if (-not $approved -or @($rules | Where-Object { $_.AccessControlType -ne 'Allow' }).Count) {
            throw 'The ACL grants access outside the owner and operating system'
        }
        'private'
    "#]).env("FIXTURE_ACL_PATH", path), ""));
    assert_eq!(text.trim(), "private");
}
