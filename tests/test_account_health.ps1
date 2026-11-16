# Health-menu public flow and portable launcher contract; no Telegram connection.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$source = [IO.File]::ReadAllText((Join-Path $root 'Manage-Accounts.ps1'), [Text.Encoding]::UTF8)
. (Join-Path $root 'account-manager/Health.ps1')
$body = [regex]::Match($source, '(?ms)^function Read-AccountNumber \{.*?^\}')
if (-not $body.Success) { throw 'Account selection function missing.' }
. ([ScriptBlock]::Create($body.Value))
$script:called = @()
function Get-Accounts { return [ordered] @{ good = 'GOOD'; busy = 'BUSY'; broken = 'TELEGRAM_SESSION_STRING' } }
function Get-SessionHealth {
    param($Key)
    $status = switch ($Key) { GOOD { 'healthy' } BUSY { 'busy' } default { 'revoked' } }
    return [pscustomobject] @{ status = $status; repairable = $true }
}
function Write-Log { param($Message) }
function Write-Hint { param($Message) }
function Write-Note { param($Message) }
function Read-Answer { param($Prompt); return '1' }
function Read-Confirmation { param($Prompt); return $true }
function Invoke-SessionGenerator {
    param($Label, $ReplaceKey, [switch] $AlreadyConfirmed)
    $script:called += "$Label|$ReplaceKey|$AlreadyConfirmed"
    $script:GeneratorExitCode = 0
}
Test-AccountSessions
if ($script:called.Count -ne 1 -or $script:called[0] -ne 'broken|TELEGRAM_SESSION_STRING|True') {
    throw 'Health menu repaired a healthy/busy account or lost the exact default key.'
}
if ($source -notmatch "'6' \{ Test-AccountSessions \}") { throw 'Menu option 6 is not wired.' }
# Cancel cannot invoke the generator.
$script:called = @()
function Read-Answer { param($Prompt); return '' }
Test-AccountSessions
if ($script:called.Count) { throw 'Cancel invoked login.' }
# Unknown health never offers login, even if an untrusted result says repairable.
function Get-SessionHealth { param($Key); return [pscustomobject] @{ status = 'unknown'; repairable = $true } }
Test-AccountSessions
if ($script:called.Count) { throw 'Unknown health invoked login.' }

# Exercise the shipped wrapper on every operating system with a synthetic child.
# Both protocol output and a failing native exit status must survive unchanged;
# the explicit logging opt-out must create no diagnostic files.
$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('tg-portable-launcher-' + [guid]::NewGuid())
[void] [IO.Directory]::CreateDirectory($fixtureRoot)
$savedBinary = $env:TELEGRAM_MCP_BINARY
$savedState = $env:TELEGRAM_STATE_DIR
$savedXdg = $env:XDG_STATE_HOME
$process = $null
try {
    $fake = Join-Path $fixtureRoot 'native-probe.ps1'
    [IO.File]::WriteAllText($fake, @'
param([Parameter(ValueFromRemainingArguments=$true)] [string[]] $Arguments)
if ($Arguments[0] -eq 'state-path') { Write-Output $env:TELEGRAM_STATE_DIR; exit 0 }
if ($Arguments[0] -ne 'serve' -or $env:TELEGRAM_MCP_LOG_FILE -ne 'off') { exit 92 }
[Console]::Out.WriteLine('portable-fixture-protocol')
[Console]::Error.WriteLine('portable-fixture-diagnostic')
exit 23
'@)
    $env:TELEGRAM_MCP_BINARY = $fake
    $env:TELEGRAM_STATE_DIR = Join-Path $fixtureRoot 'private'
    $env:XDG_STATE_HOME = Join-Path $fixtureRoot 'xdg'
    $start = [Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', (Join-Path $root 'start-mcp.ps1'), '-NoLogToFile')) {
        $start.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    [void] $process.Start()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(15000)) { $process.Kill($true); throw 'The synthetic launcher exceeded its deadline.' }
    $out = $stdout.GetAwaiter().GetResult()
    $err = $stderr.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 23) { throw "The launcher did not preserve its native failure status ($($process.ExitCode))." }
    if ($out.Trim() -ne 'portable-fixture-protocol') { throw 'The launcher changed or contaminated protocol stdout.' }
    if ($err -notmatch 'portable-fixture-diagnostic') { throw 'The launcher lost its native diagnostic output.' }
    if (@(Get-ChildItem -LiteralPath $fixtureRoot -Filter '*.log*' -File -Recurse).Count) { throw 'The portable logging opt-out wrote a diagnostic file.' }

    if (-not $IsWindows) {
        & {
            param($Root, $Fixture)
            $launcher = [IO.File]::ReadAllText((Join-Path $Root 'start-mcp.ps1'))
            foreach ($name in 'Stop-OtherInstances', 'Test-OpenedByPerson') {
                $body = [regex]::Match($launcher, "(?ms)^function $name \{.*?^\}").Value
                . ([ScriptBlock]::Create($body))
            }
            function Get-CimInstance { throw 'Unix takeover reached the Windows-only CIM API.' }
            if (-not (Test-OpenedByPerson -InputRedirected $false -HostArguments @('pwsh', '-File', 'start-mcp.ps1'))) { throw 'A manual Unix terminal was misclassified.' }
            $result = Stop-OtherInstances -Root $Fixture
            if ($result.Stopped.Count -or $result.Survived.Count) { throw 'Unix takeover reported nonexistent Windows process trees.' }
        } $root $fixtureRoot
    }
}
finally {
    if ($process) { $process.Dispose() }
    $env:TELEGRAM_MCP_BINARY = $savedBinary
    $env:TELEGRAM_STATE_DIR = $savedState
    $env:XDG_STATE_HOME = $savedXdg
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host 'Account health checks passed.'
