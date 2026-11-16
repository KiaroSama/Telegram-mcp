#Requires -Version 7.0
[CmdletBinding()]
param(
    # Persist the server's DIAGNOSTIC output (stderr) to a file under the state
    # directory. ON by default; never stdout, see the note below. Kept as an
    # explicit switch so an existing command line that passes it still works.
    [switch] $LogToFile,

    # The opt-OUT. A run that must leave nothing on disk asks for that here, or
    # sets TELEGRAM_MCP_LAUNCHER_LOG to 0/false/no/off.
    [switch] $NoLogToFile,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $ServerArguments = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:ProjectRoot = $PSScriptRoot
$exitCode = 1
$logPath = $null
$launchMutex = $null
$launchMutexHeld = $false

# Reserved for a native session-ownership refusal. Any other non-zero code is
# an ordinary failure; the launcher preserves the native process's exit status.
$script:ExitSessionHeld = 75

# STDOUT IS THE MCP PROTOCOL CHANNEL.
#
# Under the stdio transport the server speaks JSON-RPC over stdout, and those
# frames carry complete tool results: message text, contact names, chat titles,
# files listed, whatever the client asked for. Teeing stdout wrote all of it to
# disk. Only stderr -- which the server uses for diagnostics, and which
# the native diagnostic logger already bounds -- is ever persisted here.
#
# That is the whole privacy argument, and it is satisfied by the stdout/stderr
# split alone. Logging was nevertheless OPT-IN, so the default run wrote nothing
# and announced `log=unavailable` - a word that reads as "logging broke" when it
# actually meant "you did not ask". The one run an operator most needs a log for
# is the one that went wrong unexpectedly, which is exactly the run nobody
# thought to pass a flag to. So it is on by default now, and the way to get
# nothing on disk is to say so.
$script:LogDisabledReason = $null
if ($NoLogToFile.IsPresent) {
    $script:LogDisabledReason = 'disabled by -NoLogToFile'
}
elseif ($env:TELEGRAM_MCP_LAUNCHER_LOG -in @('0', 'false', 'no', 'off')) {
    $script:LogDisabledReason = 'disabled by TELEGRAM_MCP_LAUNCHER_LOG'
}
$script:LogEnabled = -not $script:LogDisabledReason

# A server run can log for days, so the file needs a ceiling as well as a count:
# the tee below rolls at LogMaxBytes keeping one previous part, and Remove-StaleFiles
# keeps the newest LogRetention runs.
$script:LogRetention = 10
$script:LogMaxBytes = 5MB

function Get-NativeBinary {
    param([string] $Root = $script:ProjectRoot)
    if ($env:TELEGRAM_MCP_BINARY) {
        if (-not (Test-Path -LiteralPath $env:TELEGRAM_MCP_BINARY -PathType Leaf)) {
            throw 'TELEGRAM_MCP_BINARY does not name an existing native executable.'
        }
        return (Get-Item -LiteralPath $env:TELEGRAM_MCP_BINARY).FullName
    }
    $names = if ($env:OS -eq 'Windows_NT') { @('telegram-mcp.exe', 'telegram-mcp') } else { @('telegram-mcp', 'telegram-mcp.exe') }
    foreach ($directory in @('', 'target/release', 'target/debug')) {
        foreach ($name in $names) {
            $relative = if ($directory) { Join-Path $directory $name } else { $name }
            $candidate = Join-Path $Root $relative
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
        }
    }
    $command = Get-Command telegram-mcp -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw 'The native telegram-mcp executable is missing. Run cargo build --locked --release, or set TELEGRAM_MCP_BINARY.'
}

function Get-StateDirectory {
    # Ask the native parser for its canonical root, using the same selector as serve.
    $sourceRoot = Get-Variable -Name ProjectRoot -Scope Script -ValueOnly -ErrorAction SilentlyContinue
    if (-not $sourceRoot) { $sourceRoot = $PSScriptRoot }
    $selector = $null
    $forwarded = Get-Variable -Name ServerArguments -ValueOnly -ErrorAction SilentlyContinue
    $forwarded = @($forwarded)
    for ($index = 0; $index -lt $forwarded.Count; $index++) {
        if ($forwarded[$index] -eq '--config') {
            if ($index + 1 -ge $forwarded.Count) { throw '--config requires a configuration path.' }
            $index++
            $selector = $forwarded[$index]
        }
        elseif ($forwarded[$index] -like '--config=*') { $selector = $forwarded[$index].Substring(9) }
    }
    if (-not $selector) {
        $selector = Get-Variable -Name envPath -ValueOnly -ErrorAction SilentlyContinue
    }
    $selection = if ($selector) { @('--config', $selector) } else { @() }
    $signature = "$sourceRoot|$env:TELEGRAM_STATE_DIR|$env:XDG_STATE_HOME|$selector|$env:TELEGRAM_ENV_FILE"
    $cached = Get-Variable -Name NativeStateDirectory -Scope Script -ValueOnly -ErrorAction SilentlyContinue
    $cachedSignature = Get-Variable -Name NativeStateDirectorySignature -Scope Script -ValueOnly -ErrorAction SilentlyContinue
    if ($cached -and $cachedSignature -eq $signature) { return $cached }
    $native = $null
    try {
        $native = Get-NativeBinary -Root $sourceRoot
        Push-Location -LiteralPath $sourceRoot
        try {
            $value = (& $native state-path @selection 2>$null | Select-Object -Last 1)
            $code = $LASTEXITCODE
        }
        finally { Pop-Location }
        if ($code -ne 0 -or -not $value -or -not [IO.Path]::IsPathRooted("$value".Trim())) {
            throw 'The native private state directory could not be resolved.'
        }
        $script:NativeStateDirectory = "$value".Trim()
        $script:NativeStateDirectorySignature = $signature
        return $script:NativeStateDirectory
    }
    catch { if ($native) { throw } }
    # This permits diagnostics before the executable is installed. Serving itself
    # still requires native discovery and cannot proceed through this fallback.
    if ($env:TELEGRAM_STATE_DIR) { return $env:TELEGRAM_STATE_DIR }
    if ($env:XDG_STATE_HOME) { return Join-Path $env:XDG_STATE_HOME 'telegram-mcp' }
    return Join-Path (Join-Path $HOME '.local/state') 'telegram-mcp'
}

function Set-OwnerOnlyAcl {
    <#
      Leave exactly one access entry on a private file or directory, and prove it.

      `icacls /inheritance:r /grant:r` was not enough, and the gap is narrow
      enough to have looked like a fix: `/inheritance:r` drops the INHERITED
      entries and `/grant:r` REPLACES the entry for the principal it names -
      every other EXPLICIT entry survives, and the tool still exits 0. A file
      carrying an explicit `BUILTIN\Users` grant therefore kept it while this
      function reported success. Measured on a GitHub Windows runner, whose
      workspace files are born with three explicit entries: all three remained.

      So the whole list is written rather than edited, and then READ BACK: the
      return value says what the object now allows, not that a call succeeded.
      A directory's entry is inheritable, which is what makes the files created
      inside one owner-only from birth. Mirrors
      the native private-state permission checks.

      Returns whether it applied, never throws: a permissions detail must not
      abort the operation it was protecting half-way.
    #>
    param([Parameter(Mandatory)] [string] $Path)
    if ($env:OS -ne 'Windows_NT') { return $false }
    try {
        $me = [Security.Principal.WindowsIdentity]::GetCurrent().User
        if (-not $me) { return $false }
        $directory = [IO.Directory]::Exists($Path)

        # A FRESH descriptor rather than the object's own: writing one read back
        # from disk also writes the AUDIT section, which needs SeSecurityPrivilege
        # and fails for an ordinary account. A new one marks only the DACL.
        $acl = if ($directory) {
            [Security.AccessControl.DirectorySecurity]::new()
        }
        else {
            [Security.AccessControl.FileSecurity]::new()
        }
        # $true, $false: protect the list and do NOT copy the inherited entries
        # into it. Without the second argument they are preserved as explicit
        # ones, which is the same leak wearing different bookkeeping.
        $acl.SetAccessRuleProtection($true, $false)
        $inheritance = if ($directory) {
            [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
        }
        else {
            [Security.AccessControl.InheritanceFlags]::None
        }
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
                $me, 'FullControl', $inheritance,
                [Security.AccessControl.PropagationFlags]::None, 'Allow'))

        $target = if ($directory) { [IO.DirectoryInfo]::new($Path) }
        else { [IO.FileInfo]::new($Path) }
        if ('System.IO.FileSystemAclExtensions' -as [type]) {
            [IO.FileSystemAclExtensions]::SetAccessControl($target, $acl)
        }
        else {
            $target.SetAccessControl($acl)  # Windows PowerShell 5.1
        }

        return (Test-OwnerOnlyAcl -Path $Path)
    }
    catch { return $false }
}

function Test-OwnerOnlyAcl {
    <#
      Whether the object's DACL names this account and nothing else.

      Read off the object rather than inferred from the call that set it: a
      tool exiting 0 says the tool ran, this says what the object allows.
    #>
    param([Parameter(Mandatory)] [string] $Path)
    if ($env:OS -ne 'Windows_NT') { return $false }
    try {
        $me = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $entries = @((Get-Acl -LiteralPath $Path).Access)
        if ($entries.Count -ne 1) { return $false }
        $held = $entries[0].IdentityReference
        if ($held -isnot [Security.Principal.SecurityIdentifier]) {
            $held = $held.Translate([Security.Principal.SecurityIdentifier])
        }
        return $held -eq $me
    }
    catch { return $false }
}
function Remove-StaleFiles {
    param(
        [Parameter(Mandatory)] [string] $Directory,
        [Parameter(Mandatory)] [string] $Filter,
        [Parameter(Mandatory)] [int] $Keep,
        [string] $Latest = ''
    )
    # Named by a UTC timestamp, so the name order IS the age order.
    $files = @(
        Get-ChildItem -LiteralPath $Directory -Filter $Filter -File -Force -ErrorAction SilentlyContinue |
            Sort-Object @{ Expression = { $_.FullName -eq $Latest } }, Name
    )
    for ($index = 0; $index -lt $files.Count - $Keep; $index++) {
        Remove-Item -LiteralPath $files[$index].FullName -Force -ErrorAction SilentlyContinue
    }
}

function Test-OpenedByPerson {
    <#
      Whether a person opened this launcher, as opposed to something starting it.

      The takeover below belongs to a person only. The always-on supervisor starts
      this script hidden with -NonInteractive, and a client that speaks stdio
      redirects its input; if either stopped the copy already running, two starters
      would stop each other's server in a loop.
    #>
    param(
        [bool] $InputRedirected = [Console]::IsInputRedirected,
        [string[]] $HostArguments = [Environment]::GetCommandLineArgs()
    )
    if ($InputRedirected) { return $false }
    return -not ($HostArguments | Where-Object { $_ -match '^[-/]noni' })
}

function Stop-OtherInstances {
    <#
      Stop every running copy of THIS checkout's server so this one can start.

      A copy is a tree - the launcher's pwsh and server, the native executable - and
      killing any one member left the rest serving the old code on the port, so each
      tree goes whole (taskkill /T). Matched by this checkout's own paths, in the
      processes that run them, so another checkout's server and an editor that merely
      has the file open are left alone. This process and its ancestors never are.

      Returns the ids stopped and the ids still alive after the wait: a survivor is
      reported, not assumed gone.
    #>
    param(
        [Parameter(Mandatory)] [string] $Root,
        [int] $WaitSeconds = 15
    )
    if (-not $IsWindows) {
        return [pscustomobject] @{ Stopped = @(); Survived = @() }
    }
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
    $targets = @($processes | Where-Object {
            $_.CommandLine -and $runners -contains $_.Name.ToLowerInvariant() -and
            -not $protected.ContainsKey([int] $_.ProcessId) -and
            (($_.Name.ToLowerInvariant() -eq 'telegram-mcp.exe' -and $_.CommandLine -match $nativePattern) -or
                ($_.Name.ToLowerInvariant() -ne 'telegram-mcp.exe' -and $_.CommandLine -match $launcherPattern))
        })
    $targetIds = @($targets | ForEach-Object { [int] $_.ProcessId })
    # The top of each tree only; its children go with it.
    foreach ($target in $targets) {
        if ($targetIds -notcontains [int] $target.ParentProcessId) {
            $null = & taskkill.exe /PID $target.ProcessId /T /F 2>$null
        }
    }

    $alive = @()
    $deadline = [DateTime]::UtcNow.AddSeconds($WaitSeconds)
    do {
        $alive = @($targetIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
        if ($alive.Count -eq 0) { break }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    return [pscustomobject]@{
        Stopped  = @($targetIds | Where-Object { $alive -notcontains $_ })
        Survived = $alive
    }
}

try {
    # Not `throw 'not requested'`: routing the opt-out through the same catch as a
    # real failure is what made both of them print the same word.
    $script:LogFailure = $null
    try {
        if (-not $script:LogEnabled) { throw [OperationCanceledException]::new('opted out') }
        $logsDirectory = Join-Path (Get-StateDirectory) 'logs'
        [void] (New-Item -ItemType Directory -Path $logsDirectory -Force)
        # The directory first, and fatally: its inheritable entries are what make
        # every log file born inside it owner-only, including the rolled parts the
        # native logger creates later, which no startup sweep can reach.
        #
        # A throw here is caught below, which sets $logPath to $null - and that is
        # what turns file logging off. Warning and carrying on would keep writing
        # diagnostics into a directory whose permissions nobody established.
        if (-not (Set-OwnerOnlyAcl -Path $logsDirectory)) {
            throw 'the log directory could not be made owner-only'
        }

        $timestamp = [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss_UTC')
        $logPath = Join-Path $logsDirectory "start-mcp_$timestamp.log"
        # Bounded, and random once the obvious names are gone. The loop had no
        # ceiling: anything that keeps making the path exist - a directory left
        # under that name, a filter driver, a permission quirk - spun it forever,
        # and a launcher that never starts is worse than one that logs elsewhere.
        for ($suffix = 1; $suffix -le 20 -and (Test-Path -LiteralPath $logPath); $suffix++) {
            $logPath = Join-Path $logsDirectory "start-mcp_${timestamp}_$suffix.log"
        }
        if (Test-Path -LiteralPath $logPath) {
            $unique = [guid]::NewGuid().ToString('N').Substring(0, 8)
            $logPath = Join-Path $logsDirectory "start-mcp_${timestamp}_$unique.log"
        }

        [IO.File]::WriteAllText($logPath, '', [Text.UTF8Encoding]::new($true))
        if (-not (Set-OwnerOnlyAcl -Path $logPath)) {
            Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
            throw 'the log file could not be made owner-only'
        }
        # `.log` AND `.log.1`: the native logger rolls a full log to `<name>.log.1`,
        # and a filter ending in `.log` never matched those, so every rotated part
        # survived every prune and the retention count bounded half the files.
        Remove-StaleFiles -Directory $logsDirectory -Filter 'start-mcp_*.log' -Keep $script:LogRetention
        Remove-StaleFiles -Directory $logsDirectory -Filter 'start-mcp_*.log.1' -Keep $script:LogRetention
    }
    catch [OperationCanceledException] {
        $logPath = $null
    }
    catch {
        $logPath = $null
        # Logging is on by default, so reaching here means it was wanted and
        # broke. Say so unconditionally: a launcher that silently loses its log
        # is the failure being reported here in the first place.
        $script:LogFailure = $_.Exception.Message
        Write-Warning "File logging failed, continuing without it: $($script:LogFailure)"
    }

    $native = Get-NativeBinary -Root $PSScriptRoot

    # Three distinct states, three distinct words. "unavailable" covered all of
    # them and told the reader nothing about which one they were in.
    $logDisplay = if ($logPath) { $logPath }
    elseif ($script:LogFailure) { "FAILED ($script:LogFailure)" }
    elseif ($script:LogDisabledReason) { $script:LogDisabledReason }
    else { 'none' }
    $startMessage = "[$([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'))] [INFO] [launcher] Starting: telegram-mcp serve; log=$logDisplay"
    # stderr, not Write-Host: the host stream is stdout once redirected, and the
    # client reads this process's stdout as the MCP protocol.
    [Console]::Error.WriteLine($startMessage)
    if ($logPath) {
        [IO.File]::AppendAllText($logPath, "$startMessage$([Environment]::NewLine)", [Text.UTF8Encoding]::new($false))
    }

    # Opened by a person: whatever copy is already running goes first, so opening the
    # launcher always ends with THIS copy serving - instead of waiting 20 seconds on
    # the session lock and exiting, which read as a crash every time.
    if ($IsWindows -and (Test-OpenedByPerson)) {
        # Same reservation as the supervisor: hold across takeover AND startup,
        # including the interval before the replacement has opened its port.
        $canonical = $PSScriptRoot.Replace('/', '\').TrimEnd('\').ToLowerInvariant()
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $digest = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($canonical)) }
        finally { $sha.Dispose() }
        $key = ([BitConverter]::ToString($digest)).Replace('-', '').ToLowerInvariant().Substring(0, 24)
        $launchMutex = [Threading.Mutex]::new($false, "Local\telegram-mcp-launch-$key")
        try { $launchMutexHeld = $launchMutex.WaitOne(30000) }
        catch [Threading.AbandonedMutexException] { $launchMutexHeld = $true }
        if (-not $launchMutexHeld) { throw 'Another launcher owns startup; no process was stopped.' }
        # Keep the named object alive as manual intent, without holding its mutex:
        # another person may still take over this manual run. Automatic starters
        # refuse any existing reservation object, including this unowned one.
        $launchMutex.ReleaseMutex()
        $launchMutexHeld = $false
        $takeover = Stop-OtherInstances -Root $PSScriptRoot
        $notes = @()
        if ($takeover.Stopped.Count) {
            $notes += "Stopped the copy of this server that was already running (pid $($takeover.Stopped -join ', ')) " +
            'so this one can start. Clients connected to it reconnect to this one.'
        }
        if ($takeover.Survived.Count) {
            throw 'The previous service did not stop; replacement was refused.'
        }
        foreach ($note in $notes) {
            $line = "[$([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'))] [INFO] [launcher] $note"
            [Console]::Error.WriteLine($line)
            if ($logPath) {
                [IO.File]::AppendAllText($logPath, "$line$([Environment]::NewLine)", [Text.UTF8Encoding]::new($false))
            }
        }
    }

    # The native server writes filtered diagnostic records itself. stdout stays
    # connected directly to the MCP client and never passes through a tee.
    $previousLogFile = $env:TELEGRAM_MCP_LOG_FILE
    $previousLogLimit = $env:TELEGRAM_MCP_LOG_MAX_BYTES
    $env:TELEGRAM_MCP_LOG_FILE = if ($logPath) { $logPath } else { 'off' }
    $env:TELEGRAM_MCP_LOG_MAX_BYTES = "$script:LogMaxBytes"
    Push-Location -LiteralPath $PSScriptRoot
    try {
        & $native serve @ServerArguments
        $nativeExitCode = Get-Variable LASTEXITCODE -ValueOnly -ErrorAction SilentlyContinue
        $exitCode = if ($null -eq $nativeExitCode) { 0 } else { [int] $nativeExitCode }
    }
    finally {
        Pop-Location
        $env:TELEGRAM_MCP_LOG_FILE = $previousLogFile
        $env:TELEGRAM_MCP_LOG_MAX_BYTES = $previousLogLimit
    }

    # A native ownership-refusal exit status means another instance of
    # THIS server already holds the Telegram session, so this one refused to
    # connect a second time rather than risk Telegram invalidating the auth key
    # for both. That is the server working, not failing - and it is the most
    # common way this launcher is run wrongly, so it gets a sentence of its own.
    # It used to leave through the same `exit 1` as a crash, and the last thing
    # an operator read was "telegram-mcp exited with code 1", which blames a toolchain that is not involved in runtime startup.
    if ($exitCode -eq $script:ExitSessionHeld) {
        $heldMessage = "[$([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'))] [INFO] " +
        '[launcher] Not started: another instance of this server is already connected ' +
        'with this Telegram session. Stop that one first, or just use it - this is not ' +
        'an error, and nothing was changed.'
        [Console]::Error.WriteLine($heldMessage)
        if ($logPath) {
            [IO.File]::AppendAllText($logPath, "$heldMessage$([Environment]::NewLine)", [Text.UTF8Encoding]::new($false))
        }
    }
    elseif ($exitCode -ne 0) {
        Write-Error "telegram-mcp exited with code $exitCode." -ErrorAction Continue
    }
}
catch {
    $stamp = [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC')
    # The MESSAGE goes to the terminal and nowhere else. Whatever threw composed
    # it, so it can carry a path, a chat title, an invite link or a credential out
    # of a connection string - and the log file is the thing an operator attaches
    # to a bug report. What is persisted is the SHAPE: the exception type and the
    # script line that raised it, by basename.
    Write-Error "[$stamp] [ERROR] [launcher] $($_.Exception.Message)" -ErrorAction Continue
    if ($logPath) {
        $type = $_.Exception.GetType().Name
        $where = if ($_.InvocationInfo -and $_.InvocationInfo.ScriptName) {
            "$(Split-Path -Leaf $_.InvocationInfo.ScriptName):$($_.InvocationInfo.ScriptLineNumber)"
        }
        else { 'unknown' }
        $record = "[$stamp] [ERROR] [launcher] $type at $where"
        [IO.File]::AppendAllText($logPath, "$record$([Environment]::NewLine)", [Text.UTF8Encoding]::new($false))
    }
}
finally {
    if ($launchMutex) {
        if ($launchMutexHeld) { $launchMutex.ReleaseMutex() }
        $launchMutex.Dispose()
    }
    $stopMessage = "[$([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'))] [INFO] [launcher] Stopped with exit code $exitCode."
    [Console]::Error.WriteLine($stopMessage)
    if ($logPath) {
        [IO.File]::AppendAllText($logPath, "$stopMessage$([Environment]::NewLine)", [Text.UTF8Encoding]::new($false))
    }
}

exit $exitCode
