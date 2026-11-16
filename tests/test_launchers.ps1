$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$projectRoot = Split-Path -Parent $PSScriptRoot
$scripts = @(
    (Join-Path $projectRoot 'start-mcp.ps1'),
    (Join-Path $projectRoot 'Manage-Accounts.ps1')
) + @(
    # The account manager dot-sources these, so a parse error in one is a broken
    # launcher just the same - and they must be checked, not assumed.
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'account-manager') -Filter '*.ps1' -File |
        Sort-Object Name | ForEach-Object { $_.FullName }
)


# A launcher is what a USER runs, which is not always one file. `start-mcp.ps1`
# is self-contained; the account manager is its entry point plus the account-manager/ files
# it dot-sources. The guarantees below belong to the launcher, so they are
# checked against the whole unit - otherwise the split would move a safety
# helper out of view and every assertion about it would quietly match nothing.
$launcherUnits = @(
    @{
        Name  = 'start-mcp.ps1'
        Files = @(Join-Path $projectRoot 'start-mcp.ps1')
    },
    @{
        Name  = 'Manage-Accounts.ps1'
        Files = @(Join-Path $projectRoot 'Manage-Accounts.ps1') + @(
            Get-ChildItem -LiteralPath (Join-Path $projectRoot 'account-manager') -Filter '*.ps1' -File |
                Sort-Object Name | ForEach-Object { $_.FullName }
        )
    }
)

function Get-UnitText {
    param([Parameter(Mandatory)] $Unit)
    return ($Unit.Files | ForEach-Object { Get-Content -LiteralPath $_ -Raw }) -join "`n"
}

foreach ($script in $scripts) {
    if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
        throw "Missing script: $script"
    }

    $parseErrors = $null
    [void] [Management.Automation.Language.Parser]::ParseFile(
        $script,
        [ref] $null,
        [ref] $parseErrors
    )
    if ($parseErrors.Count -ne 0) {
        throw "PowerShell parse errors in $script`: $($parseErrors -join '; ')"
    }
}

# Test native discovery itself with empty, unexecuted filename fixtures. Both
# launchers must find a local release/debug build without an override or PATH.
$nativeLookupRoot = Join-Path ([IO.Path]::GetTempPath()) ('tg-native-lookup-' + [guid]::NewGuid())
$savedLookupOverride = $env:TELEGRAM_MCP_BINARY
try {
    $env:TELEGRAM_MCP_BINARY = $null
    [void] (New-Item -ItemType Directory -Path (Join-Path $nativeLookupRoot 'target/release') -Force)
    [void] (New-Item -ItemType Directory -Path (Join-Path $nativeLookupRoot 'target/debug') -Force)
    $nativeName = if ($env:OS -eq 'Windows_NT') { 'telegram-mcp.exe' } else { 'telegram-mcp' }
    foreach ($directory in 'target/release', 'target/debug') {
        foreach ($name in 'telegram-mcp', 'telegram-mcp.exe') {
            [IO.File]::WriteAllText((Join-Path (Join-Path $nativeLookupRoot $directory) $name), '')
        }
    }
    foreach ($path in @((Join-Path $projectRoot 'start-mcp.ps1'), (Join-Path $projectRoot 'account-manager/Native.ps1'))) {
        $lookup = [regex]::Match([IO.File]::ReadAllText($path), '(?ms)^function Get-NativeBinary \{.*?^\}').Value
        if (-not $lookup) { throw "Native discovery helper is missing from $path" }
        & {
            param($Definition, $Root, $Name)
            . ([ScriptBlock]::Create($Definition))
            $expected = Join-Path (Join-Path $Root 'target/release') $Name
            if ((Get-NativeBinary -Root $Root) -ne $expected) { throw 'Native lookup missed the platform release binary.' }
        } $lookup $nativeLookupRoot $nativeName
    }
}
finally {
    $env:TELEGRAM_MCP_BINARY = $savedLookupOverride
    Remove-Item -LiteralPath $nativeLookupRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$launcher = Get-Content -LiteralPath $scripts[0] -Raw

# Server takeover must leave a separate login/health CLI invocation alone.
& {
    param($Root, $Launcher)
    . (Join-Path $Root 'account-manager/Server.ps1')
    $takeover = [regex]::Match($Launcher, '(?ms)^function Stop-OtherInstances \{.*?^\}').Value
    . ([ScriptBlock]::Create($takeover))
    $checkout = 'C:\fixture checkout'
    $script:processFixtures = @(
        [pscustomobject] @{ ProcessId = $PID; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = 'protected' },
        [pscustomobject] @{ ProcessId = 5000001; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = '"C:\fixture checkout\target\release\telegram-mcp.exe" login --account work' },
        [pscustomobject] @{ ProcessId = 5000002; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = '"C:\fixture checkout\target\debug\telegram-mcp.exe" session-health --key TELEGRAM_SESSION_NAME_WORK' },
        [pscustomobject] @{ ProcessId = 5000003; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = '"C:\fixture checkout\target\release\telegram-mcp.exe" serve --http 127.0.0.1:8765' },
        [pscustomobject] @{ ProcessId = 5000004; ParentProcessId = 0; Name = 'pwsh.exe'; CommandLine = 'pwsh.exe -File "C:\fixture checkout\start-mcp.ps1"' },
        [pscustomobject] @{ ProcessId = 5000005; ParentProcessId = 0; Name = 'telegram-mcp.exe'; CommandLine = 'C:\foreign\telegram-mcp.exe serve' }
    )
    $script:killed = @()
    function Get-CimInstance { param($ClassName) $script:processFixtures }
    function Get-Process { param($Id) $null }
    function taskkill.exe { $script:killed += $args }
    $trees = @(Get-ServerTrees -Root $checkout)
    if ((($trees | Sort-Object) -join ',') -ne '5000003,5000004') { throw 'Account manager matched a non-server CLI invocation.' }
    $stopped = Stop-OtherInstances -Root $checkout -WaitSeconds 0
    if ((($stopped.Stopped | Sort-Object) -join ',') -ne '5000003,5000004') { throw 'Launcher takeover matched a non-server CLI invocation.' }
} $projectRoot $launcher

# State discovery uses the native canonical root for explicit overrides and the
# selected configuration, and changes its cache when that selector changes.
$stateLookupRoot = Join-Path ([IO.Path]::GetTempPath()) ('tg-state-lookup-' + [guid]::NewGuid())
try {
    [void] [IO.Directory]::CreateDirectory((Join-Path $stateLookupRoot 'raw'))
    & {
        param($Root, $Launcher, $Fixture)
        $script:ProjectRoot = $Root
        foreach ($name in 'Get-NativeBinary', 'Get-StateDirectory') {
            $body = [regex]::Match($Launcher, "(?ms)^function $name \{.*?^\}").Value
            . ([ScriptBlock]::Create($body))
        }
        $oldState = $env:TELEGRAM_STATE_DIR
        $oldSelector = $env:TELEGRAM_ENV_FILE
        try {
            $env:TELEGRAM_ENV_FILE = $null
            $ServerArguments = @()
            $env:TELEGRAM_STATE_DIR = Join-Path $Fixture 'raw/../explicit'
            $native = Get-NativeBinary -Root $Root
            $canonical = (& $native state-path | Select-Object -Last 1).Trim()
            if ((Get-StateDirectory) -ne $canonical) { throw 'An explicit state override bypassed native canonicalization.' }
            $env:TELEGRAM_STATE_DIR = $null
            foreach ($name in 'first', 'second') {
                $selected = Join-Path $Fixture "$name.env"
                $destination = Join-Path $Fixture $name
                [IO.File]::WriteAllText($selected, "TELEGRAM_STATE_DIR='$destination'`n")
                $ServerArguments = @('--config', $selected)
                $canonical = (& $native state-path --config $selected | Select-Object -Last 1).Trim()
                if ((Get-StateDirectory) -ne $canonical) { throw 'State discovery ignored or cached the wrong configuration selector.' }
            }
        }
        finally { $env:TELEGRAM_STATE_DIR = $oldState; $env:TELEGRAM_ENV_FILE = $oldSelector }
    } $projectRoot $launcher $stateLookupRoot
}
finally { Remove-Item -LiteralPath $stateLookupRoot -Recurse -Force -ErrorAction SilentlyContinue }

# Every run of the REAL launcher below is an automated start, and says so with
# -NonInteractive. Without it, a person running these tests in their own console
# hands the launcher an interactive input, the launcher takes it for a person
# opening it (spec 013), and stops the real server running from this checkout.
$thisTest = Get-Content -LiteralPath $PSCommandPath -Raw
$unmarked = [regex]::Matches($thisTest, '& pwsh (?![^\r\n]*-NonInteractive)[^\r\n]*-File \$scripts\[0\]')
if ($unmarked.Count -ne 0) {
    throw "A test runs the real launcher without -NonInteractive: $($unmarked[0].Value)"
}

# The entry point PLUS everything it dot-sources. Reading only the entry point
# was right while it was one file; after the split, `Read-SessionString` and the
# .env writers live in account-manager/ and these assertions would match nothing - passing
# for the wrong reason instead of failing.
$manager = @(
    (Join-Path $projectRoot 'Manage-Accounts.ps1')
) + @(
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'account-manager') -Filter '*.ps1' -File |
        Sort-Object Name | ForEach-Object { $_.FullName }
) | ForEach-Object { Get-Content -LiteralPath $_ -Raw } | Join-String -Separator "`n"
if ($launcher -match '2>&1\s*\|') {
    throw 'Launcher must not pipe native output because that disables original terminal colors.'
}
# Logging moved into the native process. The launcher must preserve the protocol
# channel and pass only its private diagnostic destination to that process.
if ($launcher -match 'pythonWrapper|runpy|Get-Command uv|\.venv') {
    throw 'The launcher still depends on the removed language runtime.'
}
if ($launcher -notmatch '& \$native serve @ServerArguments') {
    throw 'The launcher does not run the native server directly with the original arguments.'
}
if ($launcher -notmatch 'TELEGRAM_MCP_LOG_FILE' -or $launcher -notmatch 'TELEGRAM_MCP_LOG_MAX_BYTES') {
    throw 'The native logger is missing its private destination or byte limit.'
}
if ($launcher -notmatch '\$env:TELEGRAM_MCP_LOG_FILE = \$previousLogFile') {
    throw 'The native logging override is not restored after server shutdown.'
}

# The account manager rewrites .env, which holds session strings. Two properties are
# not negotiable: it must never echo a secret value, and it must never overwrite the
# file without first putting a copy somewhere recoverable.
if ($manager -notmatch 'Backup-EnvFile') {
    throw 'The account manager rewrites .env with no backup step.'
}
if ($manager -match 'Write-Host[^
]*\$sessionString') {
    throw 'The account manager prints a session string to the terminal.'
}
if ($manager -notmatch 'AsSecureString') {
    throw 'The account manager reads a session string as visible input.'
}
if ($manager -notmatch '(?s)function Set-EnvValue.*?Write-FileAtomic') {
    throw 'The account manager rewrites .env in place instead of replacing it atomically.'
}
if ($manager -match '(?ms)^\s*\[IO\.File\]::WriteAllText\(\s*\r?\n\s*\$envPath') {
    throw 'The account manager still truncates .env before writing it.'
}
if ($manager -match 'Copy-Item[^\r\n]*\$envPath[^\r\n]*-Force') {
    throw 'A backup is taken with -Force, which silently replaces one taken the same second.'
}
foreach ($unit in $launcherUnits) {
    $text = Get-UnitText -Unit $unit
    # `logs/` beside the source lands inside the git checkout and inherits whatever
    # the repository directory grants; the state directory is the private one.
    if ($text -match "Join-Path \`$PSScriptRoot 'logs'") {
        throw "$($unit.Name) still logs beside its own source."
    }
    $required = @(
        'Get-StateDirectory', 'Set-OwnerOnlyAcl', 'Test-OwnerOnlyAcl', 'Remove-StaleFiles')
    foreach ($name in $required) {
        if ($text -notmatch "function $name") {
            throw "$($unit.Name) has no $name."
        }
    }
    # This used to demand the `/inheritance:r` flag, which asserted the shape of
    # the old bug rather than the guarantee: that flag drops the INHERITED
    # entries and leaves every explicit one, and icacls still exits 0. What is
    # required now is a protected list written whole, and an answer read back
    # off the file rather than taken from a return code.
    if ($text -notmatch 'SetAccessRuleProtection') {
        throw "$($unit.Name) does not write a protected DACL."
    }
    if ($text -notmatch 'return \(Test-OwnerOnlyAcl -Path \$Path\)') {
        throw "$($unit.Name) reports success without reading the file back."
    }
    # An INVOCATION, not the word: both files still describe the old approach in
    # prose, and a guard that cannot tell an explanation from a call is a guard
    # nobody can write documentation around.
    if ($text -match '(&\s+icacls|["'']icacls["''])') {
        throw "$($unit.Name) is back to editing the ACL with icacls."
    }
}
# The two launchers each carry their own copy of these four functions, and that
# duplication is deliberate: both are documented as standalone and runnable from
# a shortcut, so dot-sourcing a shared module adds a load path a double-click can
# fail on. Copies are fine only while something compares them - exactly the
# argument tests/test_console_theme.py makes for the palette carried in two
# languages.
#
# Compared as CODE, not as text: each launcher explains these functions in its own
# context and the prose has legitimately diverged, while all four bodies are
# statement-for-statement identical. The last change to Set-OwnerOnlyAcl was a real
# security fix; the next one must not be able to land in one file and not the other.
$sharedFunctions = @(
    'Get-StateDirectory', 'Set-OwnerOnlyAcl', 'Test-OwnerOnlyAcl', 'Remove-StaleFiles')
$shared = @{}
foreach ($unit in $launcherUnits) {
    $text = Get-UnitText -Unit $unit
    $shipped = $unit.Name
    $shared[$shipped] = @{}
    foreach ($name in $sharedFunctions) {
        $block = [regex]::Match($text, "(?ms)^function $name \{.*?^\}")
        if (-not $block.Success) {
            throw "$shipped declares $name but its body could not be extracted."
        }
        # Drop <# #> blocks, # comments and blank lines; keep the executable shape.
        $code = [regex]::Replace($block.Value, '(?s)<#.*?#>', '')
        $shared[$shipped][$name] = @(
            $code -split "`r?`n" |
                ForEach-Object { $_.Trim() } |
                Where-Object { $_ -and -not $_.StartsWith('#') }
        ) -join "`n"
    }
}
$left, $right = $launcherUnits.Name
foreach ($name in $sharedFunctions) {
    if ($shared[$left][$name] -ne $shared[$right][$name]) {
        throw ("$name has drifted between $left and " +
               "$right. Apply the change to both, or give them " +
               "different names so the difference is deliberate and visible.")
    }
}

# Every .env this manager creates is restricted before anything is written into
# it: the file ends up holding session strings, and a session string is the
# account, with no password and no second factor.
foreach ($creation in [regex]::Matches($manager, '(?ms)WriteAllText\(\$envPath.{0,300}')) {
    if ($creation.Value -notmatch 'Set-OwnerOnlyAcl -Path \$envPath') {
        throw 'Manage-Accounts.ps1 creates a .env without restricting it to its owner.'
    }
}

if ($launcher -notmatch 'LogMaxBytes') {
    throw 'The launcher log has no size ceiling; a long-running server fills the disk.'
}
# Under the stdio transport stdout IS the MCP protocol channel and carries whole
# tool results. Teeing it wrote the user's messages, contacts and files to disk.
if ($launcher -match 'sys\.stdout\s*=\s*Tee') {
    throw 'The launcher tees stdout, which is the MCP protocol channel.'
}
if ($launcher -notmatch 'TELEGRAM_MCP_LOG_FILE') {
    throw 'The launcher no longer configures private native diagnostic logging.'
}
if ($launcher -notmatch 'LogToFile') {
    throw 'Persisting output to disk is not opt-in.'
}

$nativeFixture = Join-Path ([IO.Path]::GetTempPath()) ("tg-native-" + [guid]::NewGuid() + '.ps1')
[IO.File]::WriteAllText($nativeFixture, @'
param([Parameter(ValueFromRemainingArguments=$true)] [string[]] $Arguments)
if ($Arguments.Count -and $Arguments[0] -eq 'state-path') {
    [Console]::Out.WriteLine((Join-Path $env:XDG_STATE_HOME 'telegram-mcp'))
    exit 0
}
if ($env:TG_TEST_EXPECT_LOG_DISABLED -eq '1' -and $env:TELEGRAM_MCP_LOG_FILE -ne 'off') {
    [Console]::Error.WriteLine('The launcher did not forward its nonempty logging-disable setting.')
    exit 91
}
[Console]::Out.WriteLine('fake-normal-output')
[Console]::Error.WriteLine('fake-error-output')
exit 0
'@, [Text.UTF8Encoding]::new($false))
$savedNativeBinary = $env:TELEGRAM_MCP_BINARY
$env:TELEGRAM_MCP_BINARY = $nativeFixture

# XDG_STATE_HOME points the launcher at a throwaway directory, so this asserts the
# real path resolution rather than reaching into the operator's own state.
$stateHome = Join-Path ([IO.Path]::GetTempPath()) ("tg-state-" + [guid]::NewGuid())
$originalStateHome = $env:XDG_STATE_HOME
$env:XDG_STATE_HOME = $stateHome
$originalLauncherLog = $env:TELEGRAM_MCP_LAUNCHER_LOG
$env:TELEGRAM_MCP_LAUNCHER_LOG = '1'
$logsDirectory = Join-Path $stateHome 'telegram-mcp/logs'

$originalPath = $env:PATH
$originalPathExt = $env:PATHEXT
$newLogs = @()
try {
    $env:PATH = (Join-Path $PSScriptRoot 'fixtures') + [IO.Path]::PathSeparator + $originalPath
    $env:PATHEXT = ".PS1;$originalPathExt"
    $output = & pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $scripts[0] 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Launcher exited with $LASTEXITCODE`: $($output -join [Environment]::NewLine)"
    }
    $newLogs = @(
        Get-ChildItem -LiteralPath $logsDirectory -Filter 'start-mcp_*.log' -File |
            Select-Object -ExpandProperty FullName
    )
    if ($newLogs.Count -ne 1) {
        throw "Expected one launcher log under $logsDirectory, found $($newLogs.Count)."
    }

    $log = Get-Content -LiteralPath $newLogs[0] -Raw
    foreach ($expected in '[INFO] [launcher]') {
        if ($log -notmatch [regex]::Escape($expected)) {
            throw "Launcher log is missing: $expected"
        }
    }
    if ($log -match 'fake-normal-output|fake-error-output') {
        throw 'The launcher wrote stdout to disk; stdout is the MCP protocol channel.'
    }
}
finally {
    $env:PATH = $originalPath
    $env:PATHEXT = $originalPathExt
    if ($null -eq $originalStateHome) {
        Remove-Item -LiteralPath Env:XDG_STATE_HOME -ErrorAction SilentlyContinue
    }
    else {
        $env:XDG_STATE_HOME = $originalStateHome
    }
    if ($null -eq $originalLauncherLog) {
        Remove-Item -LiteralPath Env:TELEGRAM_MCP_LAUNCHER_LOG -ErrorAction SilentlyContinue
    }
    else {
        $env:TELEGRAM_MCP_LAUNCHER_LOG = $originalLauncherLog
    }
    Remove-Item -LiteralPath $stateHome -Recurse -Force -ErrorAction SilentlyContinue
}

# A default run LOGS, and an opted-out run writes nothing. Both directions are
# asserted here because the interesting failure was neither: logging used to be
# opt-in, so the ordinary run produced no file and announced `log=unavailable` -
# which reads as "logging broke" and sent at least one debugging session looking
# for a fault that did not exist.
$quietStateHome = Join-Path ([IO.Path]::GetTempPath()) ("tg-quiet-" + [guid]::NewGuid())
$originalStateHome = $env:XDG_STATE_HOME
$originalPath = $env:PATH
$originalPathExt = $env:PATHEXT
try {
    $env:XDG_STATE_HOME = $quietStateHome
    $env:PATH = "$PSScriptRoot\fixtures;$originalPath"
    $env:PATHEXT = ".PS1;$originalPathExt"
    Remove-Item -LiteralPath Env:TELEGRAM_MCP_LAUNCHER_LOG -ErrorAction SilentlyContinue

    # 1. The plain run, no flags and no environment: a log must appear, and the
    #    announced path must be the file that actually exists.
    $defaultErr = & pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $scripts[0] 2>&1
    $quietLogs = Join-Path $quietStateHome 'telegram-mcp/logs'
    $written = @(Get-ChildItem -LiteralPath $quietLogs -File -ErrorAction SilentlyContinue)
    if ($written.Count -lt 1) {
        throw 'The launcher wrote no log on a default run; logging is meant to be on unless refused.'
    }
    $announced = ($defaultErr | Out-String)
    if ($announced -match 'log=unavailable') {
        throw 'The launcher still reports log=unavailable, the wording this test exists to retire.'
    }
    if ($announced -notmatch [regex]::Escape($written[0].Name)) {
        throw "The launcher announced a log path that is not the file it created ($($written[0].Name))."
    }
    Write-Output 'ok  a default run writes a log and names the file it wrote'

    # 2. The refusal, both ways in. Nothing on disk, and a reason rather than a
    #    word that could equally mean a failure.
    foreach ($refusal in @(@('switch', @('-NoLogToFile'), $null), @('env', @(), 'off'))) {
        $offHome = Join-Path ([IO.Path]::GetTempPath()) ("tg-off-" + [guid]::NewGuid())
        $env:XDG_STATE_HOME = $offHome
        if ($refusal[2]) { $env:TELEGRAM_MCP_LAUNCHER_LOG = $refusal[2] }
        else { Remove-Item -LiteralPath Env:TELEGRAM_MCP_LAUNCHER_LOG -ErrorAction SilentlyContinue }
        try {
            $env:TG_TEST_EXPECT_LOG_DISABLED = '1'
            $offErr = (& pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $scripts[0] @($refusal[1]) 2>&1 | Out-String)
            if ($LASTEXITCODE -ne 0) { throw "Logging opt-out was not accepted by the native child: $offErr" }
            $offLogs = Join-Path $offHome 'telegram-mcp/logs'
            $leaked = @(Get-ChildItem -LiteralPath $offLogs -File -Recurse -ErrorAction SilentlyContinue)
            if ($leaked.Count -ne 0) {
                throw "The launcher wrote $($leaked.Count) log file(s) after being told not to ($($refusal[0]))."
            }
            if ($offErr -notmatch 'log=disabled by') {
                throw "A refused run must say it was disabled, not merely omit the path ($($refusal[0]))."
            }
        }
        finally {
            Remove-Item -LiteralPath Env:TG_TEST_EXPECT_LOG_DISABLED -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $offHome -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    $env:XDG_STATE_HOME = $quietStateHome
    Write-Output 'ok  a refused run writes nothing and says which refusal applied'
}
finally {
    $env:PATH = $originalPath
    $env:PATHEXT = $originalPathExt
    if ($null -eq $originalStateHome) {
        Remove-Item -LiteralPath Env:XDG_STATE_HOME -ErrorAction SilentlyContinue
    }
    else {
        $env:XDG_STATE_HOME = $originalStateHome
    }
    Remove-Item -LiteralPath $quietStateHome -Recurse -Force -ErrorAction SilentlyContinue
}

# Retention, proved against the shipped function rather than by eleven real launches.
$retentionDirectory = Join-Path ([IO.Path]::GetTempPath()) ("tg-retention-" + [guid]::NewGuid())
[void] (New-Item -ItemType Directory -Path $retentionDirectory)
try {
    $prune = [regex]::Match($launcher, '(?ms)^function Remove-StaleFiles \{.*?^\}')
    if (-not $prune.Success) { throw 'Could not extract Remove-StaleFiles from the launcher.' }
    . ([ScriptBlock]::Create($prune.Value))

    foreach ($index in 1..15) {
        $name = 'start-mcp_2026-01-{0:d2}_00-00-00_UTC.log' -f $index
        [IO.File]::WriteAllText((Join-Path $retentionDirectory $name), 'x')
    }
    Remove-StaleFiles -Directory $retentionDirectory -Filter 'start-mcp_*.log' -Keep 10
    $kept = @(Get-ChildItem -LiteralPath $retentionDirectory -Filter 'start-mcp_*.log' -File |
            Sort-Object Name)
    if ($kept.Count -ne 10) { throw "Retention kept $($kept.Count) logs, expected 10." }
    # The OLDEST five go, not the newest: a log is only useful while it is recent.
    if ($kept[0].Name -notmatch '2026-01-06') {
        throw "Retention deleted the wrong end: oldest kept is $($kept[0].Name)."
    }
    Write-Output 'ok  launcher logs are pruned to the newest ten'
}
finally {
    Remove-Item -LiteralPath $retentionDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

# Takeover (spec 013): a launcher a person opens stops the copy already running, and
# only this checkout's copy. Proved against the shipped functions on real processes:
# two dummies that name the native server executable under two different roots, the way a real copy does.
$takeoverRoot = Join-Path ([IO.Path]::GetTempPath()) ("tg-takeover-" + [guid]::NewGuid())
$otherRoot = Join-Path ([IO.Path]::GetTempPath()) ("tg-other-" + [guid]::NewGuid())
$dummies = @()
try {
    foreach ($name in 'Stop-OtherInstances', 'Test-OpenedByPerson') {
        $function = [regex]::Match($launcher, "(?ms)^function $name \{.*?^\}")
        if (-not $function.Success) { throw "Could not extract $name from the launcher." }
        . ([ScriptBlock]::Create($function.Value))
    }

    $shell = (Get-Process -Id $PID).Path
    foreach ($root in $takeoverRoot, $otherRoot) {
        $dummies += Start-Process -FilePath $shell -WindowStyle Hidden -PassThru -ArgumentList (
            "-NoProfile -NonInteractive -Command Start-Sleep -Seconds 120 # $root\telegram-mcp.exe")
    }
    $mine, $theirs = $dummies

    $result = Stop-OtherInstances -Root $takeoverRoot
    if ($result.Stopped -notcontains $mine.Id) {
        throw "The running copy was not reported stopped (stopped: $($result.Stopped -join ', '))."
    }
    if (-not $mine.WaitForExit(15000)) { throw 'The running copy is still alive after the takeover.' }
    if ($theirs.HasExited) { throw "The takeover stopped another checkout's server." }
    Write-Output 'ok  opening the launcher stops this checkout''s running copy and nothing else'

    # Who counts as a person: a console with its own input and no -NonInteractive. The
    # supervisor starts the launcher hidden with -NonInteractive and a stdio client
    # redirects its input - neither may stop anything, or two starters loop forever.
    if (-not (Test-OpenedByPerson -InputRedirected $false -HostArguments @('pwsh.exe', '-File', 'start-mcp.ps1'))) {
        throw 'A launcher opened by hand did not count as opened by a person.'
    }
    if (Test-OpenedByPerson -InputRedirected $false -HostArguments @('pwsh.exe', '-NonInteractive', '-File', 'x')) {
        throw 'A -NonInteractive start (the supervisor) counted as a person.'
    }
    if (Test-OpenedByPerson -InputRedirected $true -HostArguments @('pwsh.exe', '-File', 'x')) {
        throw 'A start with redirected input (a stdio client) counted as a person.'
    }
    Write-Output 'ok  only a launcher a person opened takes over'
}
finally {
    foreach ($dummy in $dummies) {
        if (-not $dummy.HasExited) { & taskkill.exe /PID $dummy.Id /T /F 2>&1 | Out-Null }
    }
}

Remove-Item -LiteralPath $nativeFixture -Force
$env:TELEGRAM_MCP_BINARY = $savedNativeBinary
Write-Output 'Launcher checks passed.'
