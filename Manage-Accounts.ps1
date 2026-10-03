#Requires -Version 5.1
<#
    Add, remove and inspect the Telegram accounts this server exposes.

    Accounts live in `.env` as TELEGRAM_SESSION_STRING_<LABEL> lines, one per
    account, plus the unsuffixed TELEGRAM_SESSION_STRING which the server labels
    "default". This menu edits exactly those lines and leaves every other line in
    the file byte-for-byte alone.

    Two rules shape the whole script, because a session string is a live login to
    a Telegram account and is worth more than a password:

      * it is never printed, never logged, and never passed on a command line -
        it is read as a SecureString and held only long enough to write it;
      * `.env` is copied to .env.backup-<UTC> before any rewrite, so a mistake
        here costs one rename rather than every configured account.

    Adding an account needs a session string, which comes from
    `session_string_generator.py`. This script offers to run that for you, but the
    QR scan or the phone code is yours to complete - nothing here logs you in.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$exitCode = 0
$script:LogPath = $null
$envPath = Join-Path $PSScriptRoot '.env'
$script:ProjectRoot = $PSScriptRoot

# How many of each are kept. Both hold private material - a log names the accounts
# on this machine, a backup holds a full login to every one of them - so an
# unbounded pile of either turns one readable directory into a standing leak.
$script:LogRetention = 10
$script:EnvBackupRetention = 1
$script:MaxBackupCollisions = 100

# --- the pieces this launcher is made of --------------------------------------
#
# Dot-sourced rather than imported as a module: these run in THIS scope, so the
# `$script:` state above and `$envPath` are visible to them without being passed
# around. `$PSScriptRoot` is per-FILE, which is why every function that resolves
# the project root - the ones that call the venv - stayed in this file.
foreach ($piece in 'FileSafety', 'EnvFile', 'Console', 'Server', 'Health') {
    $module = Join-Path $PSScriptRoot (Join-Path 'account-manager' "$piece.ps1")
    if (-not (Test-Path -LiteralPath $module)) {
        Write-Host "Missing $module - this launcher needs the lib folder beside it." -ForegroundColor Red
        exit 1
    }
    . $module
}

# --- actions -----------------------------------------------------------------

function Show-Accounts {
    <#
      The numbers are not decoration: `Remove-Account` asks for one, so the
      listing and the choice must agree. The caller passes the very dictionary it
      will index, rather than each re-reading `.env`, because two reads are two
      chances for the numbering to mean different things.
    #>
    param($Accounts)

    $accounts = if ($null -ne $Accounts) { $Accounts } else { Get-Accounts }
    if ($accounts.Count -eq 0) {
        Write-Host 'No accounts are configured yet.' -ForegroundColor Yellow
        Write-Host 'Choose "Add an account" to configure the first one.'
        return
    }
    Write-Host ''
    Write-Host "Configured accounts ($($accounts.Count)):" -ForegroundColor Cyan
    $number = 0
    foreach ($label in $accounts.Keys) {
        $number++
        $note = if ($label -eq 'default') { '  (default configuration)' } else { '' }
        Write-Host ("  {0,2}. {1,-16} {2}{3}" -f $number, $label, $accounts[$label], $note)
    }
    Write-Host 'Configured does not mean authorized. Option 6 checks current session health.' -ForegroundColor Yellow
    if ($accounts.Count -gt 1) {
        Write-Host ''
        Write-Host 'Every tool call names its account with account=<label>, as it did with one.' -ForegroundColor Yellow
    }
}

function Invoke-SessionGenerator {
    param(
        [string] $Label,
        # Add-Account has just asked whether to generate one. Asking again here is
        # the same question twice in a row, which is what a caller reports as noise.
        [switch] $AlreadyConfirmed,
        [string] $ReplaceKey,
        [long] $ExpectedUserId = 0,
        [string] $ExpectedConfigDigest
    )

    $script:GeneratorExitCode = 1
    Write-Host ''
    if ($ReplaceKey) { Write-Host 'Log in to the same account you are repairing.' }
    else { Write-Host 'Log in as the account you want to ADD, not one already configured.' }
    if ($Label) {
        Write-Hint "It will save the result as '$Label' - press Enter when it offers to."
    }
    Write-Host ''
    if (-not $AlreadyConfirmed -and -not (Read-Confirmation 'Run the session generator now?')) { return }

    # No --qr / --phone here on purpose: without a flag the generator asks, so the
    # choice always matches whatever methods it actually supports.
    $script = 'session_string_generator.py'
    # The label it would otherwise ask for. Passing it is what stops the same
    # question being put twice, once by each half of this flow.
    $arguments = @($script)
    if ($ReplaceKey) {
        $arguments += @('--replace-key', $ReplaceKey, '--no-echo')
        if ($ExpectedConfigDigest) { $arguments += @('--expected-config-digest', $ExpectedConfigDigest) }
        if ($ExpectedUserId -gt 0) { $arguments += @('--expected-user-id', "$ExpectedUserId") }
    }
    elseif ($Label) { $arguments += @('--label', $Label, '--no-echo') }
    $python = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'

    Push-Location -LiteralPath $PSScriptRoot
    try {
        $script:GeneratorExitCode = $null
        if (Test-Path -LiteralPath $python -PathType Leaf) {
            # Straight to the interpreter. `uv run` would rebuild and reinstall the
            # project first whenever a source file has changed, printing build and
            # wheel-install progress on top of the login prompt.
            & $python @arguments
            $script:GeneratorExitCode = $LASTEXITCODE
        }
        else {
            $uv = Get-Command uv -ErrorAction SilentlyContinue
            if (-not $uv) {
                throw "Neither .venv\Scripts\python.exe nor uv was found. Create the virtual environment, or install uv, then try again."
            }
            Write-Hint 'No .venv found - falling back to uv, which may build the project first.'
            # UV_LINK_MODE is uv's own advice for the hardlink warning it prints when
            # the cache and the target sit on different filesystems.
            $previousLinkMode = $env:UV_LINK_MODE
            $env:UV_LINK_MODE = 'copy'
            try {
                & $uv.Path run --quiet @arguments
                $script:GeneratorExitCode = $LASTEXITCODE
            }
            finally { $env:UV_LINK_MODE = $previousLinkMode }
        }
    }
    finally { Pop-Location }

    if ($script:GeneratorExitCode -ne 0) {
        Write-Host ''
        Write-Failure 'The generator did not save a replacement.'
        Write-Host 'The previous configuration is unchanged. Resolve the reported problem before trying again.'

    }
}

function Test-SessionString {
    <#
      Ask Telethon whether this parses as a session, rather than guessing from its
      length. A 42-character paste sailed past the old `length -lt 40` check and
      was written to .env as a working account; `StringSession` rejects it outright.

      The value goes in on STDIN, never as an argument: a command line is visible
      to anything that can list processes.
    #>
    param([Parameter(Mandatory)] [AllowEmptyString()] [string] $Value)

    # An empty value is a session with no auth key; say so rather than throwing on
    # the parameter binding, which is what a Mandatory [string] does to ''.
    if ([string]::IsNullOrWhiteSpace($Value)) { return 'empty' }

    $python = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $python -PathType Leaf)) { return 'unchecked' }

    $probe = @'
import sys
from telethon.sessions import StringSession
raw = sys.stdin.read().strip()
try:
    session = StringSession(raw)
except Exception:
    print("invalid")
else:
    print("valid" if session.auth_key and session.dc_id else "empty")
'@
    try {
        $verdict = ($Value | & $python -c $probe 2>$null | Select-Object -Last 1)
        if ($LASTEXITCODE -ne 0) { return 'unchecked' }
        return "$verdict".Trim()
    }
    catch { return 'unchecked' }
}


function Add-Account {
    $accounts = Get-Accounts
    Write-Host ''
    $label = Read-Label 'Label for the new account (e.g. work, personal) - blank to cancel'
    if (-not $label) { Write-Host 'Cancelled.'; return }

    if ($accounts.Contains($label)) {
        Write-Host "An account labelled '$label' already exists ($($accounts[$label]))." -ForegroundColor Yellow
        if (-not (Read-Confirmation 'Replace its session string?')) { Write-Host 'Cancelled.'; return }
    }

    Write-Host ''
    Write-Host (Get-Painted -Text 'A session string authorises full access to that Telegram account.' -ColorName 'NoteYellow')
    if (Read-Confirmation 'Do you need to generate one first?') {
        if ($accounts.Contains($label)) {
            Invoke-SessionGenerator -Label $label -ReplaceKey $accounts[$label] -AlreadyConfirmed
        }
        else { Invoke-SessionGenerator -Label $label -AlreadyConfirmed }

        # The generator can write the line itself now. Asking for a paste after it
        # already did would be asking someone to copy a 350-character secret across
        # a terminal for no reason - which is how a mis-paste got saved once.
        if ($script:GeneratorExitCode -eq 0 -and (Get-Accounts).Contains($label)) {
            Write-Host ''
            Write-Host "The generator saved '$label' to .env." -ForegroundColor Green
            Write-Host 'A running server picks this up on its own - no restart needed.' -ForegroundColor Cyan
            return
        }
        Write-Note 'The generator did not save it. No account was changed.'
        return
    }

    $sessionString = Read-SessionString 'Paste the session string (input stays hidden)'
    if ([string]::IsNullOrWhiteSpace($sessionString)) {
        Write-Host 'Nothing was pasted; no change made.' -ForegroundColor Yellow
        return
    }
    switch (Test-SessionString -Value $sessionString) {
        'valid' { }
        'empty' {
            Write-Failure 'That parses as a session but carries no auth key - it is an empty session.'
            Write-Host 'Nothing was saved.'
            return
        }
        'invalid' {
            Write-Failure 'Telethon cannot read that as a session string, so it would never load.'
            Write-Host 'Check you copied the whole line the generator printed. Nothing was saved.'
            return
        }
        default {
            Write-Note 'Could not verify the session string (no .venv to check it with).'
            if (-not (Read-Confirmation 'Save it unverified?')) { Write-Host 'Cancelled.'; return }
        }
    }

    $backup = Backup-EnvFile
    $key = "TELEGRAM_SESSION_STRING_$($label.ToUpperInvariant())"
    Set-EnvValue -Key $key -Value $sessionString
    $sessionString = $null

    Write-Log "Added account '$label' as $key"
    Write-Host ''
    Write-Host "Added '$label'." -ForegroundColor Green
    if ($backup) { Write-Host "Previous .env kept as $(Split-Path -Leaf $backup)" }
    Write-Host 'A running server picks this up on its own - no restart needed.' -ForegroundColor Cyan
    if ((Get-Accounts).Count -gt 1) {
        Write-Host ''
        Write-Host 'You now have more than one account, so write tools will require' -ForegroundColor Yellow
        Write-Host "account=<label> from here on - for example account=$label." -ForegroundColor Yellow
    }
}


function Read-AccountNumber {
    <#
      Pick an account by the number the listing just printed.

      Typing the label was the old way and it made "list it, read the name, go
      back, type it exactly" a four-step job for a one-word answer - and an
      underscore in the stored form that the eye reads as a space is enough to
      make the typed version miss.

      Indexes the caller's own dictionary, so the numbering cannot disagree with
      what was shown. Returns the label, or $null for cancel.
    #>
    param(
        [Parameter(Mandatory)] $Accounts,
        [Parameter(Mandatory)] [string] $Prompt
    )
    $labels = @($Accounts.Keys)
    while ($true) {
        $raw = Read-Answer -Prompt $Prompt
        if ($null -eq $raw) { return $null }
        $trimmed = $raw.Trim()
        if ($trimmed -eq '') { return $null }
        if ($trimmed -match '^[0-9]+$') {
            $index = [int] $trimmed
            if ($index -ge 1 -and $index -le $labels.Count) { return $labels[$index - 1] }
        }
        Write-Note "Enter a number from 1 to $($labels.Count)."
    }
}

function Remove-SecretChatKeys {
    <#
      Delete the account's secret-chat key store along with its .env line.

      These are two stores and only one used to be cleared. The store outlives
      the account, so removing a label and reusing it for a different person left
      the new account holding the old one's chats - keys that decrypt nothing,
      under a name that now means someone else.

      Best effort by design: the account is already gone from `.env` by this
      point, and a leftover store is a nuisance rather than a failure, so a
      failure here must not abort a removal that has already happened.

      The history file beside it goes too. It holds decrypted message text, and
      leaving one behind for an account that was just removed would be the one
      place this machine still remembers a conversation the owner ended.
    #>
    param([Parameter(Mandatory)] [string] $Label)

    $root = Join-Path (Get-StateDirectory) 'secret-chats'
    # The media file holds received files' keys; the owner file would refuse the
    # next account given this label ("keys belong to a different account").
    foreach ($item in @($Label, "$Label-history.json", "$Label-media.json", "$Label.owner.json")) {
        $path = Join-Path $root $item
        if (-not (Test-Path -LiteralPath $path)) { continue }
        try {
            Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
            Write-Log "Removed secret-chat state '$item' for '$Label'"
        }
        catch {
            Write-Log "Could not remove secret-chat state '$item' for '$Label': $($_.Exception.Message)" -Level WARNING
        }
    }
}

function Remove-Account {
    $accounts = Get-Accounts
    if ($accounts.Count -eq 0) { Write-Host 'There is nothing to remove.' -ForegroundColor Yellow; return }

    # The same dictionary the listing numbered, so choice N is the account
    # printed as N. Re-reading .env here would be a second source of truth for
    # the one thing that must not be wrong in a delete.
    Show-Accounts -Accounts $accounts
    Write-Host ''
    $label = Read-AccountNumber -Accounts $accounts -Prompt 'Number to remove - blank to cancel'
    if (-not $label) { Write-Host 'Cancelled.'; return }
    if ($accounts.Count -eq 1) {
        Write-Host ''
        Write-Host 'This is the only account. Removing it leaves the server unable to start' -ForegroundColor Yellow
        Write-Host 'until another one is configured.' -ForegroundColor Yellow
    }

    Write-Host ''
    Write-Host "This removes $($accounts[$label]) from .env." -ForegroundColor Yellow
    Write-Host 'The Telegram session itself stays authorised - to truly revoke it, end the'
    Write-Host 'session from Telegram: Settings > Devices.'
    if (-not (Read-Confirmation "Remove '$label'?")) { Write-Host 'Cancelled.'; return }

    $backup = Backup-EnvFile
    Remove-EnvKey -Key $accounts[$label]
    Remove-SecretChatKeys -Label $label
    Write-Log "Removed account '$label' ($($accounts[$label]))"
    Write-Host ''
    Write-Host "Removed '$label'." -ForegroundColor Green
    if ($backup) { Write-Host "Previous .env kept as $(Split-Path -Leaf $backup)" }
    Write-Host 'A running server picks this up on its own - no restart needed.' -ForegroundColor Cyan
}

function Move-AccountState {
    <#
      Move everything the server stored under a label - secret-chat keys, history
      and files, ghost mode, proxy route, approvals, contact aliases - to another
      label (telegram_mcp.account_rename). All or nothing; throws on a refusal or a
      failure, having changed nothing. Returns what moved.
    #>
    param(
        [Parameter(Mandatory)] [string] $From,
        [Parameter(Mandatory)] [string] $To
    )
    $python = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
        throw "No .venv\Scripts\python.exe, so the account's stored state cannot be moved; nothing was renamed."
    }
    Push-Location -LiteralPath $PSScriptRoot
    try {
        $output = @(& $python -m telegram_mcp.account_rename (Get-StateDirectory) $From $To 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    }
    finally { Pop-Location }
    if ($code -ne 0) { throw "Moving the stored state failed: $(($output -join ' ').Trim())" }
    return $output
}

function Rename-Account {
    $accounts = Get-Accounts
    if ($accounts.Count -eq 0) { Write-Host 'There is nothing to rename.' -ForegroundColor Yellow; return }

    # By number, like Remove-Account, from the same dictionary the listing numbered.
    Show-Accounts -Accounts $accounts
    Write-Host ''
    $from = Read-AccountNumber -Accounts $accounts -Prompt 'Number to rename - blank to cancel'
    if (-not $from) { Write-Host 'Cancelled.'; return }
    if ($from -eq 'default') {
        Write-Host "'default' comes from the unsuffixed TELEGRAM_SESSION_STRING and cannot be" -ForegroundColor Yellow
        Write-Host 'renamed here. Remove it and add it back under a label instead.' -ForegroundColor Yellow
        return
    }

    $to = Read-Label "New label for '$from'"
    if (-not $to) { Write-Host 'Cancelled.'; return }
    if ($to -eq $from) { Write-Host 'That is already its label.' -ForegroundColor Yellow; return }
    if ($accounts.Contains($to)) { Write-Host "'$to' is already taken." -ForegroundColor Yellow; return }

    $oldKey = $accounts[$from]

    # The PREFIX decides what the value MEANS. A file-based account is defined by
    # TELEGRAM_SESSION_NAME_*, and rewriting it as TELEGRAM_SESSION_STRING_* hands
    # the server a session PATH where it expects a session STRING - so the rename
    # succeeds, says so, and the account silently stops loading.
    $prefix = if ($oldKey.StartsWith('TELEGRAM_SESSION_NAME_')) {
        'TELEGRAM_SESSION_NAME_'
    }
    else { 'TELEGRAM_SESSION_STRING_' }
    $newKey = "$prefix$($to.ToUpperInvariant())"

    Write-Host ''
    Write-Host "Renames the label '$from' to '$to' in .env and secrets.md, and moves what the server"
    Write-Host 'stored under it: secret-chat keys and history, ghost mode, proxy route, approvals, aliases.'
    Write-Host 'The Telegram account itself is not changed.'
    if (@(Get-ServerTrees -Root $PSScriptRoot).Count -gt 0) {
        Write-Host 'The MCP server is running: it is stopped for the rename and started again.' -ForegroundColor Yellow
    }
    if (-not (Read-Confirmation "Rename '$from' to '$to'?")) { Write-Host 'Cancelled.'; return }

    # Stopped first: the server holds the secret-chat key store open under the old
    # label. ponytail: the always-on supervisor waits at least 5 s before restarting a
    # stopped server, and the moves below take well under that; Start-TelegramServer
    # takes over whatever copy started meanwhile anyway.
    $stopped = Stop-TelegramServer -Root $PSScriptRoot
    $backup = $null
    $registry = $false
    try {
        $moved = @(Move-AccountState -From $from -To $to)
        $backup = Backup-EnvFile
        try {
            # One write. The value is never read into a variable here - it moves inside
            # the transform, so nothing in this scope ever holds a session string.
            Rename-EnvKey -From $oldKey -To $newKey
        }
        catch {
            $null = Move-AccountState -From $to -To $from
            throw
        }
        try {
            $registry = Rename-SecretsEntry -Path (Join-Path $PSScriptRoot 'secrets.md') `
                -OldKey $oldKey -NewKey $newKey -OldLabel $from -NewLabel $to
        }
        catch {
            # The account already works under the new label; only the registry lags.
            Write-Note "secrets.md was not updated ($($_.Exception.Message)); rename its '## $oldKey' heading by hand."
        }
    }
    finally {
        if ($stopped) {
            $serverId = Start-TelegramServer -Root $PSScriptRoot
            Write-Host "MCP server started again in the background (pid $serverId)." -ForegroundColor Cyan
        }
    }

    Write-Log "Renamed account '$from' to '$to' ($($moved.Count) stored item(s) moved)"
    Write-Host ''
    Write-Host "Renamed '$from' to '$to'." -ForegroundColor Green
    foreach ($item in $moved) { Write-Host "  moved: $item" }
    if ($registry) { Write-Host "  secrets.md: '## $oldKey' is now '## $newKey'" }
    if ($backup) { Write-Host "Previous .env kept as $(Split-Path -Leaf $backup)" }
    Write-Host "Tools now take account=$to." -ForegroundColor Cyan
}

# --- menu --------------------------------------------------------------------

$script:MenuItems = [ordered] @{
    '1' = 'List configured accounts'
    '2' = 'Add an account'
    '3' = 'Remove an account'
    '4' = 'Rename an account'
    '5' = 'Generate a session string only'
    '6' = 'Check session health / re-login'
}

function Show-Menu {
    Write-Host ''
    Write-Host (Get-Painted -Text 'Telegram MCP account manager:' -ColorName 'LightBlue')
    foreach ($key in $script:MenuItems.Keys) {
        Write-Host "  $(Get-Painted -Text "$key." -ColorName 'LightBlue') $($script:MenuItems[$key])"
    }
    Write-Host ''
}

Start-Logging
Write-Log 'Account manager started'

try {
    if (-not (Test-Path -LiteralPath $envPath -PathType Leaf)) {
        Write-Host ''
        Write-Note "No .env file exists at $envPath."
        Write-Host 'It also has to hold TELEGRAM_API_ID and TELEGRAM_API_HASH, which this menu'
        Write-Host 'does not manage - copy .env.example first, fill those in, then come back.'
        if (Read-Confirmation 'Create an empty .env now so accounts can be added?') {
            [IO.File]::WriteAllText($envPath, '', [Text.UTF8Encoding]::new($false))
            # Before anything is put in it: this file ends up holding session
            # strings, and a session string is the account.
            if (-not (Set-OwnerOnlyAcl -Path $envPath)) {
                Remove-Item -LiteralPath $envPath -Force -ErrorAction SilentlyContinue
                Write-Host 'The .env could not be made owner-only, so it was not created.'
                exit 1
            }
            Write-Log 'Created an empty .env'
        }
        else {
            Write-Host 'Nothing was changed.'
            exit 0
        }
    }

    while (-not $script:Quitting) {
        Show-Menu
        # -NoBack, and deliberately: the main menu has no previous step, so
        # advertising back=0 here would promise something that cannot happen.
        # This is FFmWiz's own rule, kept rather than reinvented.
        $choice = Read-Answer -Prompt 'Selection' -NoBack
        if ($script:Quitting) { break }
        if ([string]::IsNullOrEmpty($choice)) { continue }

        switch ($choice) {
            '1' { Show-Accounts }
            '2' { Add-Account }
            '3' { Remove-Account }
            '4' { Rename-Account }
            '5' { Invoke-SessionGenerator }
            '6' { Test-AccountSessions }
            default { Write-Failure "Enter a menu number from 1 to $($script:MenuItems.Count), or exit." }
        }
        if ($script:Quitting) { break }
    }
}
catch {
    $exitCode = 1
    # Shown in full, persisted as its shape. This log records account operations,
    # so an exception message here can carry a label, a path or part of a session
    # string - and the file outlives the terminal.
    Write-Host ''
    Write-Failure "Failed: $($_.Exception.Message)"
    $where = if ($_.InvocationInfo -and $_.InvocationInfo.ScriptName) {
        "$(Split-Path -Leaf $_.InvocationInfo.ScriptName):$($_.InvocationInfo.ScriptLineNumber)"
    }
    else { 'unknown' }
    Write-Log "$($_.Exception.GetType().Name) at $where" -Level ERROR
}
finally {
    Write-Log "Account manager stopped with exit code $exitCode"
    if ($script:LogPath) { Write-Hint "Log: $script:LogPath" }
}

exit $exitCode
