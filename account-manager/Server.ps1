<#
    Stopping and starting this checkout's MCP server around a change that must not
    happen under a running one (spec 025: renaming an account label moves the files
    the server holds open under that label).

    Dot-sourced like the other pieces, so it runs in the launcher's scope. The root is
    passed in rather than read from `$PSScriptRoot`, which here would name this folder.
#>

function Get-ServerTrees {
    <#
      The top process of every running copy of this checkout's server - the same
      match `start-mcp.ps1`'s own takeover uses: PowerShell and the native executable whose
      command line names this checkout's start-mcp.ps1 or telegram-mcp.exe. This process and
      its ancestors never count.
    #>
    param([Parameter(Mandatory)] [string] $Root)
    $processes = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
    $parents = @{}
    foreach ($process in $processes) { $parents[[int] $process.ProcessId] = [int] $process.ParentProcessId }
    $protected = @{}
    $walk = $PID
    while ($walk -and -not $protected.ContainsKey($walk)) {
        $protected[$walk] = $true
        $walk = $parents[$walk]
    }
    $checkout = [regex]::Escape($Root.TrimEnd('\', '/'))
    $launcherPattern = $checkout + '[\\/]start-mcp\.ps1(?:"|\s|$)'
    $nativePattern = $checkout + '[\\/](?:target[\\/](?:release|debug)[\\/])?telegram-mcp\.exe"?\s+serve(?:\s|$)'
    $runners = 'pwsh.exe', 'powershell.exe', 'telegram-mcp.exe'
    $matched = @($processes | Where-Object {
            $_.CommandLine -and $runners -contains $_.Name.ToLowerInvariant() -and
            -not $protected.ContainsKey([int] $_.ProcessId) -and
            (($_.Name.ToLowerInvariant() -eq 'telegram-mcp.exe' -and $_.CommandLine -match $nativePattern) -or
                ($_.Name.ToLowerInvariant() -ne 'telegram-mcp.exe' -and $_.CommandLine -match $launcherPattern))
        })
    $ids = @($matched | ForEach-Object { [int] $_.ProcessId })
    return @($matched | Where-Object { $ids -notcontains [int] $_.ParentProcessId } |
            ForEach-Object { [int] $_.ProcessId })
}

function Stop-TelegramServer {
    <#
      Stop every running copy, whole trees (taskkill /T): killing one member left the
      rest serving on the port. Returns $true when something was running, $false when
      nothing was; throws when a copy survives the wait, because the caller is about
      to move files that copy still holds.
    #>
    param(
        [Parameter(Mandatory)] [string] $Root,
        [int] $WaitSeconds = 15
    )
    $tops = @(Get-ServerTrees -Root $Root)
    if ($tops.Count -eq 0) { return $false }
    foreach ($id in $tops) { $null = & taskkill.exe /PID $id /T /F 2>$null }
    $deadline = [DateTime]::UtcNow.AddSeconds($WaitSeconds)
    while (@(Get-ServerTrees -Root $Root).Count -gt 0) {
        if ([DateTime]::UtcNow -ge $deadline) {
            throw "The MCP server did not stop within $WaitSeconds s; nothing was renamed."
        }
        Start-Sleep -Milliseconds 250
    }
    Write-Log "Stopped the MCP server (pid $($tops -join ', '))"
    return $true
}

function Start-TelegramServer {
    <#
      Start the server again, hidden. Deliberately WITHOUT -NonInteractive: that makes
      it the launcher a person opened, so it takes over any copy the always-on
      supervisor started in the meantime and the supervisor then yields to it.
    #>
    param([Parameter(Mandatory)] [string] $Root)
    $shell = Get-Command pwsh.exe -ErrorAction SilentlyContinue
    if (-not $shell) { throw 'PowerShell 7 (pwsh.exe) is required to restart the server.' }
    $launcher = Join-Path $Root 'start-mcp.ps1'
    $native = Get-NativeBinary -Root $Root
    $address = (& $native bind-address --config $envPath | Select-Object -Last 1)
    if ($LASTEXITCODE -ne 0 -or -not $address) { throw 'The native service address could not be resolved.' }
    $address = "$address".Trim()
    $process = Start-Process -FilePath $shell.Source -WindowStyle Hidden -WorkingDirectory $Root -PassThru `
        -ArgumentList ('-NoLogo -NoProfile -File "{0}" --config "{1}" --http {2}' -f $launcher, $envPath, $address)
    Write-Log "Started the MCP server again (pid $($process.Id))"
    return $process.Id
}
