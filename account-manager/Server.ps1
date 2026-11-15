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
    $pattern = [regex]::Escape($Root.TrimEnd('\', '/')) + '[\\/](start-mcp\.ps1|(?:target[\\/](?:release|debug)[\\/])?telegram-mcp\.exe)'
    $runners = 'pwsh.exe', 'powershell.exe', 'telegram-mcp.exe'
    $matched = @($processes | Where-Object {
            $_.CommandLine -and $runners -contains $_.Name.ToLowerInvariant() -and
            -not $protected.ContainsKey([int] $_.ProcessId) -and $_.CommandLine -match $pattern
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
    $hostName = if ($env:MCP_HOST) { $env:MCP_HOST } else { '127.0.0.1' }
    $port = if ($env:MCP_PORT) { $env:MCP_PORT } else { '8765' }
    foreach ($line in Get-EnvLines) {
        if (-not $env:MCP_HOST -and $line -match '^\s*MCP_HOST\s*=\s*["'']?([^"''#\s]+)') { $hostName = $Matches[1] }
        if (-not $env:MCP_PORT -and $line -match '^\s*MCP_PORT\s*=\s*["'']?(\d+)') { $port = $Matches[1] }
    }
    $address = if ($hostName.Contains(':')) { "[${hostName}]:$port" } else { "${hostName}:$port" }
    $process = Start-Process -FilePath $shell.Source -WindowStyle Hidden -WorkingDirectory $Root -PassThru `
        -ArgumentList ('-NoLogo -NoProfile -File "{0}" --http {1}' -f $launcher, $address)
    Write-Log "Started the MCP server again (pid $($process.Id))"
    return $process.Id
}
