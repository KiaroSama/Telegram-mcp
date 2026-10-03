<# Current authorization checks and owner-driven repair. Never reads credentials into this scope. #>

function Get-SessionHealth {
    param([Parameter(Mandatory)] [string] $Key)
    $python = Join-Path $script:ProjectRoot '.venv\Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
        return [pscustomobject] @{ status = 'unknown'; repairable = $false }
    }
    $stdout = $null
    $stderr = $null
    $process = $null
    try {
        $folder = Join-Path (Get-StateDirectory) 'health-checks'
        [IO.Directory]::CreateDirectory($folder) | Out-Null
        if (-not (Set-OwnerOnlyAcl -Path $folder)) { throw 'Private health output directory unavailable.' }
        $id = [Guid]::NewGuid().ToString('N')
        $stdout = Join-Path $folder "$id.out"
        $stderr = Join-Path $folder "$id.err"
        $arguments = @('-m', 'telegram_mcp.session_health', '--env-file', "`"$envPath`"", '--key', $Key)
        $process = Start-Process -FilePath $python -ArgumentList $arguments -WorkingDirectory $script:ProjectRoot `
            -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
        if (-not $process.WaitForExit(45000)) {
            & taskkill /PID $process.Id /T /F 2>$null | Out-Null
            $process.WaitForExit(5000) | Out-Null
            throw 'Health check exceeded its deadline.'
        }
        if ($process.ExitCode -ne 0) { throw 'Health check failed.' }
        $health = ([IO.File]::ReadAllText($stdout, [Text.Encoding]::UTF8).Trim()) | ConvertFrom-Json
        if ($health.status -notin @('healthy', 'unauthorized', 'revoked', 'invalid', 'network', 'unknown', 'busy')) {
            throw 'Unrecognized health result.'
        }
        return $health
    }
    catch { return [pscustomobject] @{ status = 'unknown'; repairable = $false } }
    finally {
        if ($process) { $process.Dispose() }
        if ($stdout -and (Test-Path -LiteralPath $stdout)) { Remove-Item -LiteralPath $stdout -Force }
        if ($stderr -and (Test-Path -LiteralPath $stderr)) { Remove-Item -LiteralPath $stderr -Force }
    }
}

function Test-AccountSessions {
    $accounts = Get-Accounts
    if ($accounts.Count -eq 0) { Write-Note 'No accounts are configured.'; return }
    $repairable = [ordered] @{}
    $healthByLabel = @{}
    Write-Host ''
    Write-Host 'Checking current Telegram authorization (no login prompts)...' -ForegroundColor Cyan
    foreach ($label in $accounts.Keys) {
        $health = Get-SessionHealth -Key $accounts[$label]
        $color = if ($health.status -eq 'healthy') { 'Green' } else { 'Yellow' }
        Write-Host ("  {0,-20} {1}" -f $label, $health.status) -ForegroundColor $color
        Write-Log "Session health for '$label': $($health.status)"
        # Derive eligibility from the status, never from a truthy remote JSON field.
        if ($health.status -in @('unauthorized', 'revoked', 'invalid')) {
            $repairable[$label] = $accounts[$label]
            $healthByLabel[$label] = $health
        }
    }
    Write-Hint 'Network/unknown/busy means unverified, not invalid. No credential was changed.'
    if ($repairable.Count -eq 0) { return }
    Write-Host ''
    Write-Host 'Accounts that need re-login:' -ForegroundColor Yellow
    $number = 0
    foreach ($label in $repairable.Keys) {
        $number++
        Write-Host ("  {0}. {1}" -f $number, $label)
    }
    $label = Read-AccountNumber -Accounts $repairable -Prompt 'Number to re-login - blank to cancel'
    if (-not $label) { Write-Host 'Cancelled. Nothing was changed.'; return }
    Write-Note 'Log in to the SAME Telegram account. Old device-bound secret chats cannot be restored by a new login; their files will be kept.'
    if (-not (Read-Confirmation "Re-login for '$label'?")) { return }
    $parameters = @{ Label = $label; ReplaceKey = $repairable[$label]; AlreadyConfirmed = $true }
    $health = $healthByLabel[$label]
    if ($health.PSObject.Properties['config_digest']) { $parameters.ExpectedConfigDigest = [string] $health.config_digest }
    if ($health.PSObject.Properties['user_id']) { $parameters.ExpectedUserId = [long] $health.user_id }
    Invoke-SessionGenerator @parameters
    if ($script:GeneratorExitCode -eq 0) {
        Write-Host "Replacement saved for '$label'. The server verifies it before activation." -ForegroundColor Green
    }
}
