#Requires -Version 7.0
param()

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$utf8 = [Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = $utf8
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8

function Get-NativeBinary {
    if ($env:TELEGRAM_MCP_BINARY) {
        if (-not (Test-Path -LiteralPath $env:TELEGRAM_MCP_BINARY -PathType Leaf)) {
            throw 'TELEGRAM_MCP_BINARY does not name an existing native executable.'
        }
        return (Get-Item -LiteralPath $env:TELEGRAM_MCP_BINARY).FullName
    }
    $names = if ($IsWindows) { 'telegram-mcp.exe', 'telegram-mcp' } else { 'telegram-mcp', 'telegram-mcp.exe' }
    foreach ($directory in 'bin', '.', 'target/release', 'target/debug') {
        foreach ($name in $names) {
            $candidate = Join-Path $PSScriptRoot $directory $name
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    $command = Get-Command telegram-mcp -CommandType Application -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw 'The native telegram-mcp executable is missing. Place it in bin/ or set TELEGRAM_MCP_BINARY.'
}

try {
    $native = Get-NativeBinary
    Push-Location -LiteralPath $PSScriptRoot
    try {
        # The native command loads .env, resolves private state, and writes its launcher log.
        & $native manage @args
        $code = $LASTEXITCODE
    }
    finally { Pop-Location }
    exit $code
}
catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
