param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $Arguments
)

# The updater's Python test invocation: `uv run python -m pytest ...`. It exits
# with TELEGRAM_MCP_FAKE_PYTEST_EXIT (default 0) so a caller can tell "the tests
# ran and failed" from "the tests never ran".
if ($Arguments.Count -ge 4 -and
    $Arguments[0] -eq 'run' -and
    $Arguments[1] -eq 'python' -and
    $Arguments[2] -eq '-m' -and
    $Arguments[3] -eq 'pytest') {
    [Console]::Out.WriteLine('fake-pytest-output')
    $pytestExit = 0
    if (-not [string]::IsNullOrWhiteSpace($env:TELEGRAM_MCP_FAKE_PYTEST_EXIT)) {
        $pytestExit = [int] $env:TELEGRAM_MCP_FAKE_PYTEST_EXIT
    }
    exit $pytestExit
}

# `uv run python <wrapper.py> <log path> <max bytes> <main.py> [server args...]`.
#
# A FILE, never `-c`. Handing multi-line Python containing double quotes to a
# native executable as one argument is where Windows PowerShell 5.1 differs from
# pwsh 7: 5.1 drops the inner quotes and `python -c` dies with a SyntaxError
# before a single client starts. `powershell.exe` IS 5.1, so the launcher was
# dead for every double-click, Start-Process and embedding client while it kept
# working in the pwsh session it was developed in. The `-c` rejection below is
# what stops that from coming back.
if ($Arguments.Count -lt 6 -or
    $Arguments[0] -ne 'run' -or
    $Arguments[1] -ne 'python' -or
    $Arguments[2] -eq '-c' -or
    [IO.Path]::GetExtension($Arguments[2]) -ne '.py' -or
    $Arguments[4] -notmatch '^\d+$' -or
    [IO.Path]::GetFileName($Arguments[5]) -ne 'main.py') {
    throw "Unexpected uv arguments: $($Arguments -join ' ')"
}

$logPath = $Arguments[3]
[Console]::Out.WriteLine('fake-normal-output')
[Console]::Error.WriteLine('fake-error-output')
# Only stderr reaches the file, because only stderr is teed: stdout is the MCP
# protocol channel and carries whole tool results.
[IO.File]::AppendAllText(
    $logPath,
    "fake-error-output$([Environment]::NewLine)",
    [Text.UTF8Encoding]::new($false)
)
