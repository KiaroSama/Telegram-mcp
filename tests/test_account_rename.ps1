# Renaming an account label in the launcher (spec 025): the secrets.md entry, the
# order of stop / move / .env / restart, and the removal of every secret-chat sidecar.
#
# Runs against a throwaway sandbox: its own .env, secrets.md and XDG_STATE_HOME. The
# server stop/start and the Python state move are stubbed; tests/test_account_rename.py
# covers the move itself.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$projectRoot = Split-Path -Parent $PSScriptRoot
$sandbox = Join-Path ([IO.Path]::GetTempPath()) ("tg-rename-" + [guid]::NewGuid())
[void] (New-Item -ItemType Directory -Path $sandbox)
$savedState = $env:XDG_STATE_HOME

function Assert-That([bool] $Condition, [string] $Message) { if (-not $Condition) { throw $Message } }

try {
    $env:XDG_STATE_HOME = Join-Path $sandbox 'state'
    foreach ($piece in 'FileSafety', 'EnvFile', 'Console', 'Server') {
        . (Join-Path $projectRoot "account-manager\$piece.ps1")
    }
    $launcher = Get-Content -LiteralPath (Join-Path $projectRoot 'Manage-Accounts.ps1') -Raw
    foreach ($name in 'Remove-SecretChatKeys', 'Move-AccountState', 'Rename-Account') {
        $block = [regex]::Match($launcher, "(?ms)^function $name \{.*?^\}").Value
        Assert-That ([bool] $block) "Manage-Accounts.ps1 has no $name."
        . ([scriptblock]::Create($block))
    }
    $script:LogPath = $null
    $script:EnvBackupRetention = 5
    $script:MaxBackupCollisions = 100
    $envPath = Join-Path $sandbox '.env'
    $secretsPath = Join-Path $sandbox 'secrets.md'
    $PSScriptRoot = $sandbox

    # --- secrets.md: only the renamed entry changes, its value stays -------------
    $registry = @(
        '# Local Secrets'
        ''
        '## TELEGRAM_SESSION_STRING_OLD'
        "Purpose: Telethon session of the account 'old' (telegram-mcp)"
        'Value:'
        '```text'
        'VALUE-OLD'
        '```'
        ''
        '## TELEGRAM_SESSION_STRING_OTHER'
        "Purpose: Telethon session of the account 'other' - see account 'old' too"
        'VALUE-OTHER'
    ) -join "`n"
    [IO.File]::WriteAllText($secretsPath, $registry, [Text.UTF8Encoding]::new($false))
    $changed = Rename-SecretsEntry -Path $secretsPath -OldKey 'TELEGRAM_SESSION_STRING_OLD' `
        -NewKey 'TELEGRAM_SESSION_STRING_NEW' -OldLabel 'old' -NewLabel 'new'
    $after = [IO.File]::ReadAllText($secretsPath)
    Assert-That $changed 'Rename-SecretsEntry reported nothing to do.'
    Assert-That ($after -match '(?m)^## TELEGRAM_SESSION_STRING_NEW$') 'The heading was not renamed.'
    Assert-That ($after -match "account 'new' \(telegram-mcp\)") 'The purpose line still names the old label.'
    Assert-That ($after -match 'VALUE-OLD') 'The value did not survive.'
    Assert-That ($after -match "see account 'old' too") 'Another entry was edited.'
    Assert-That ($after -notmatch '(?m)^## TELEGRAM_SESSION_STRING_OLD') 'The old heading is still there.'
    $missing = Rename-SecretsEntry -Path $secretsPath -OldKey 'TELEGRAM_SESSION_STRING_NONE' `
        -NewKey 'X' -OldLabel 'none' -NewLabel 'x'
    Assert-That (-not $missing) 'A key with no entry was reported as renamed.'
    Write-Host 'ok  secrets.md: the one entry is renamed, its value and the others untouched'

    # --- rename order: a refused state move leaves .env alone, server restarted ----
    [IO.File]::WriteAllLines($envPath, @('TELEGRAM_SESSION_STRING_OLD=abc', 'TELEGRAM_SESSION_STRING_KEEP=def'))
    $script:calls = [Collections.Generic.List[string]]::new()
    function Read-AccountNumber { param($Accounts, $Prompt) 'old' }
    function Read-Label { param($Prompt) 'new' }
    function Read-Confirmation { param($Prompt) $true }
    function Show-Accounts { param($Accounts) }
    function Get-ServerTrees { param($Root) @(1) }
    function Stop-TelegramServer { param($Root) $script:calls.Add('stop'); $true }
    function Start-TelegramServer { param($Root) $script:calls.Add('start'); 42 }
    function Backup-EnvFile { $null }
    function Move-AccountState { param($From, $To) $script:calls.Add("move $From>$To"); throw 'Refused: taken' }

    $threw = $false
    try { Rename-Account } catch { $threw = $true }
    Assert-That $threw 'A refused state move did not stop the rename.'
    Assert-That ((Get-Content $envPath) -contains 'TELEGRAM_SESSION_STRING_OLD=abc') '.env changed after a refused move.'
    Assert-That (($script:calls -join ',') -eq 'stop,move old>new,start') "Wrong order: $($script:calls -join ',')"
    Write-Host 'ok  a refused state move changes nothing and the server comes back'

    # --- a failed .env write moves the state back ----------------------------------
    $script:calls.Clear()
    function Move-AccountState { param($From, $To) $script:calls.Add("move $From>$To"); @('secret-chats/old') }
    function Rename-EnvKey { param($From, $To) throw 'disk full' }
    $threw = $false
    try { Rename-Account } catch { $threw = $true }
    Assert-That $threw 'A failed .env write was not reported.'
    Assert-That (($script:calls -join ',') -eq 'stop,move old>new,move new>old,start') "No rollback: $($script:calls -join ',')"
    Write-Host 'ok  a failed .env write moves the stored state back'

    # --- removal clears every secret-chat sidecar ----------------------------------
    $chats = Join-Path $env:XDG_STATE_HOME 'telegram-mcp\secret-chats'
    [void] (New-Item -ItemType Directory -Path (Join-Path $chats 'gone') -Force)
    foreach ($file in 'gone-history.json', 'gone-media.json', 'gone.owner.json', 'kept-media.json') {
        [IO.File]::WriteAllText((Join-Path $chats $file), '{}')
    }
    Remove-SecretChatKeys -Label 'gone'
    $left = @(Get-ChildItem -LiteralPath $chats -Name)
    Assert-That (($left -join ',') -eq 'kept-media.json') "Left behind after removal: $($left -join ',')"
    Write-Host 'ok  removing an account clears its keys, history, file keys and owner record'

    # --- no server for this sandbox checkout means nothing to stop -----------------
    . (Join-Path $projectRoot 'account-manager\Server.ps1')
    Assert-That (@(Get-ServerTrees -Root $sandbox).Count -eq 0) 'A server was found for a checkout that has none.'
    Assert-That (-not (Stop-TelegramServer -Root $sandbox)) 'Stopping a server that is not running reported one.'
    Write-Host 'ok  a checkout with no running server has nothing to stop'
}
finally {
    $env:XDG_STATE_HOME = $savedState
    Remove-Item -LiteralPath $sandbox -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host 'Account rename checks passed.' -ForegroundColor Green
