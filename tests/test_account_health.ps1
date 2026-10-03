# Health-menu public flow with controlled probe/login; no Telegram or process launch.
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
Write-Host 'ok  exact-key repair, busy/healthy/unknown exclusion and cancellation'
