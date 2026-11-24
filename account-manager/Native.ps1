# Native executable discovery shared by the account manager actions.

function Get-NativeBinary {
    param([string] $Root = $script:ProjectRoot)
    if ($env:TELEGRAM_MCP_BINARY) {
        if (-not (Test-Path -LiteralPath $env:TELEGRAM_MCP_BINARY -PathType Leaf)) {
            throw 'TELEGRAM_MCP_BINARY does not name an existing native executable.'
        }
        return (Get-Item -LiteralPath $env:TELEGRAM_MCP_BINARY).FullName
    }
    $names = if ($env:OS -eq 'Windows_NT') { @('telegram-mcp.exe', 'telegram-mcp') } else { @('telegram-mcp', 'telegram-mcp.exe') }
    foreach ($directory in @('bin', '', 'target/release', 'target/debug')) {
        foreach ($name in $names) {
            $relative = if ($directory) { Join-Path $directory $name } else { $name }
            $candidate = Join-Path $Root $relative
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    $command = Get-Command telegram-mcp -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw 'The native telegram-mcp executable is missing. Place it in bin/, run cargo build --locked --release, or set TELEGRAM_MCP_BINARY.'
}

function Test-SessionString {
    <#
      Ask the native session parser whether this parses as a session, rather than guessing from its
      length. A 42-character paste sailed past the old `length -lt 40` check and
      was written to .env as a working account; the strict binary decoder rejects it outright.

      The value goes in on STDIN, never as an argument: a command line is visible
      to anything that can list processes.
    #>
    param([Parameter(Mandatory)] [AllowEmptyString()] [string] $Value)

    # An empty value is a session with no auth key; say so rather than throwing on
    # the parameter binding, which is what a Mandatory [string] does to ''.
    if ([string]::IsNullOrWhiteSpace($Value)) { return 'empty' }

    try {
        $native = Get-NativeBinary -Root $script:ProjectRoot
        $verdict = ($Value | & $native session-validate 2>$null | Select-Object -Last 1)
        if ($LASTEXITCODE -ne 0 -or "$verdict".Trim() -notin @('valid', 'invalid', 'empty')) { return 'unchecked' }
        return "$verdict".Trim()
    }
    catch { return 'unchecked' }
}
