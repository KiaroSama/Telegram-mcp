//! Native Windows process discovery. Only a checkout launcher or its serve process can seed a stop.
use super::StoppedService;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsString, c_void},
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_NO_MORE_FILES, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{
        CREATE_NEW_CONSOLE, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    },
};

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    name: [u16; 260],
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    capacity: u16,
    buffer: *const u16,
}

// These two Win32 modules and the NT query are outside the installed windows-sys feature set.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> HANDLE;
    fn Process32FirstW(snapshot: HANDLE, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: HANDLE, entry: *mut ProcessEntry) -> i32;
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        buffer: *mut c_void,
        size: u32,
        actual: *mut u32,
    ) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn CommandLineToArgvW(command_line: *const u16, count: *mut i32) -> *mut *mut u16;
}

struct Process {
    pid: u32,
    parent: u32,
    name: String,
    start: Option<u64>,
}

fn snapshot() -> Result<Vec<Process>> {
    // SAFETY: TH32CS_SNAPPROCESS takes no process-specific inputs.
    let handle = unsafe { CreateToolhelp32Snapshot(2, 0) };
    ensure!(
        handle != INVALID_HANDLE_VALUE,
        "Cannot inspect Windows process trees: {}",
        io::Error::last_os_error()
    );
    // SAFETY: the checked snapshot handle belongs to this call and will close on drop.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    // SAFETY: ProcessEntry is a C POD struct with no invalid zero values.
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = size_of::<ProcessEntry>() as u32;
    // SAFETY: the snapshot is live and entry advertises its exact initialized buffer size.
    let mut more = unsafe { Process32FirstW(handle.as_raw_handle(), &mut entry) } != 0;
    let mut processes = Vec::new();
    while more {
        let end = entry
            .name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.name.len());
        let recorded_start =
            open(entry.pid, PROCESS_QUERY_LIMITED_INFORMATION).and_then(|h| started(&h));
        processes.push(Process {
            pid: entry.pid,
            parent: entry.parent,
            name: String::from_utf16_lossy(&entry.name[..end]).to_ascii_lowercase(),
            start: recorded_start,
        });
        // SAFETY: same live snapshot and reusable C buffer as in the first call.
        more = unsafe { Process32NextW(handle.as_raw_handle(), &mut entry) } != 0;
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(error).context("Cannot finish inspecting Windows process trees");
    }
    Ok(processes)
}

fn open(pid: u32, rights: u32) -> Option<OwnedHandle> {
    // SAFETY: a non-null process handle is owned by this call and closed once on drop.
    let handle = unsafe { OpenProcess(rights, 0, pid) };
    if handle.is_null() {
        None
    } else {
        Some(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn command_args(handle: &OwnedHandle) -> Option<Vec<OsString>> {
    // ProcessCommandLineInformation returns the UTF-16 text in the caller's buffer.
    const COMMAND_LINE: u32 = 60;
    let mut size = 0;
    // SAFETY: the process handle is live; a zero-sized query only returns the required size.
    unsafe {
        NtQueryInformationProcess(
            handle.as_raw_handle(),
            COMMAND_LINE,
            null_mut(),
            0,
            &mut size,
        )
    };
    if !(size_of::<UnicodeString>() as u32..=128 * 1024).contains(&size) {
        return None;
    }
    let mut memory = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: `memory` is initialized, correctly aligned, and has at least `size` writable bytes.
    if unsafe {
        NtQueryInformationProcess(
            handle.as_raw_handle(),
            COMMAND_LINE,
            memory.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } < 0
    {
        return None;
    }
    // SAFETY: the call succeeded and allocated a buffer with room and alignment for UnicodeString.
    let text = unsafe { &*memory.as_ptr().cast::<UnicodeString>() };
    let length = usize::from(text.length);
    let offset = (text.buffer as usize).checked_sub(memory.as_ptr() as usize)?;
    if length == 0
        || !length.is_multiple_of(2)
        || !offset.is_multiple_of(2)
        || offset < size_of::<UnicodeString>()
        || offset.checked_add(length)? > memory.len() * size_of::<usize>()
    {
        return None;
    }
    // SAFETY: offset and length were checked against the allocation, with UTF-16 alignment.
    let buffer = unsafe {
        std::slice::from_raw_parts(
            memory.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
            length / 2,
        )
    };
    if buffer.contains(&0) {
        return None;
    }
    let terminated: Vec<_> = buffer.iter().copied().chain([0]).collect();
    let mut count = 0;
    // SAFETY: text is nul-terminated in our allocation, and count is writable.
    let argv = unsafe { CommandLineToArgvW(terminated.as_ptr(), &mut count) };
    if argv.is_null() {
        return None;
    }
    // SAFETY: CommandLineToArgvW returns count valid pointers to nul-terminated argument strings.
    let args = unsafe { std::slice::from_raw_parts(argv, count as usize) }
        .iter()
        .map(|&arg| {
            // SAFETY: each pointer names a nul-terminated argument in the still-live allocation.
            let mut len = 0;
            unsafe {
                while *arg.add(len) != 0 {
                    len += 1;
                }
                OsString::from_wide(std::slice::from_raw_parts(arg, len))
            }
        })
        .collect();
    // SAFETY: the returned allocation and its argument contents are now fully copied.
    unsafe { LocalFree(argv.cast()) };
    Some(args)
}

fn started(process: &OwnedHandle) -> Option<u64> {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the live handle has query rights and all four outputs are distinct writable FILETIMEs.
    (unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } != 0)
        .then_some(u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime))
}

fn image(process: &OwnedHandle) -> Option<PathBuf> {
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    // SAFETY: the image query takes a live handle and a UTF-16 buffer of the advertised length.
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    } == 0
    {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(
        path.get(..length as usize)?,
    )))
}

/// Compare two filesystem paths by exact OS canonical identity, failing closed
/// on any canonicalization error so deleted or inaccessible paths never match
/// another checkout.
fn same_path(left: &Path, right: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(left), std::fs::canonicalize(right)),
        (Ok(a), Ok(b)) if a == b
    )
}

fn launcher_args<'a>(root: &Path, mut args: &'a [OsString]) -> Option<&'a [OsString]> {
    while let Some((option, rest)) = args.split_first() {
        match option.to_str()?.to_ascii_lowercase().as_str() {
            "-file" | "-f" | "-fi" | "-fil" => {
                let (script, forwarded) = rest.split_first()?;
                return same_path(Path::new(script), &root.join("start-mcp.ps1"))
                    .then_some(forwarded);
            }
            "-nologo" | "-nol" | "-noprofile" | "-nop" | "-noninteractive" | "-noni"
            | "-noexit" | "-noe" | "-interactive" | "-i" | "-sta" | "-mta" => args = rest,
            "-executionpolicy" | "-ep" | "-ex" | "-windowstyle" | "-w" | "-workingdirectory"
            | "-wd" | "-inputformat" | "-inp" | "-if" | "-outputformat" | "-o" | "-of" => {
                args = rest.get(1..)?;
            }
            // A positional script, command, or unrecognized option ends the ownership scan.
            _ => return None,
        }
    }
    None
}

fn config_argument(args: &[OsString]) -> Option<PathBuf> {
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            break;
        }
        if arg == "--config" {
            return args.get(index + 1).map(PathBuf::from);
        }
        if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--config=")) {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn service_args<'a>(root: &Path, image: &Path, args: &'a [OsString]) -> Option<&'a [OsString]> {
    let name = image.file_name()?.to_str()?.to_ascii_lowercase();
    let args = args.get(1..)?;
    match name.as_str() {
        "pwsh.exe" | "powershell.exe" => launcher_args(root, args),
        "telegram-mcp.exe" | "telegram-mcp" => {
            let command = match args.first()? {
                arg if arg == "--config" => args.get(2),
                arg if arg.as_encoded_bytes().starts_with(b"--config=") => args.get(1),
                arg => Some(arg),
            };
            (command.is_some_and(|argument| argument == "serve")
                && ["", "bin", "target/release", "target/debug"]
                    .into_iter()
                    .any(|dir| same_path(&root.join(dir).join(&name), image)))
            .then_some(args)
        }
        _ => None,
    }
}

struct OwnedProcess {
    handle: OwnedHandle,
    start: u64,
    config: Option<PathBuf>,
}

fn owned_processes(root: &Path, terminate: bool) -> Result<Vec<OwnedProcess>> {
    let processes = snapshot()?;
    let parents: HashMap<_, _> = processes.iter().map(|p| (p.pid, p.parent)).collect();
    let mut protected = HashSet::new();
    let mut current = std::process::id();
    while current != 0 && protected.insert(current) {
        current = parents.get(&current).copied().unwrap_or(0);
    }
    let mut selected = HashMap::<u32, OwnedProcess>::new();
    let query = PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE;
    for process in &processes {
        if protected.contains(&process.pid)
            || !matches!(
                process.name.as_str(),
                "telegram-mcp" | "telegram-mcp.exe" | "pwsh.exe" | "powershell.exe"
            )
        {
            continue;
        }
        let Some(handle) = open(process.pid, query) else {
            continue;
        };
        let Some(start) = started(&handle) else {
            continue;
        };
        let Some((image, args)) = image(&handle).zip(command_args(&handle)) else {
            continue;
        };
        let Some(args) = service_args(root, &image, &args) else {
            continue;
        };
        if !terminate {
            return Ok(vec![OwnedProcess {
                handle,
                start,
                config: None,
            }]);
        }
        // Hold the inspected handle until acquiring a second one that has termination rights.
        let terminator = open(process.pid, query | PROCESS_TERMINATE)
            .context("This checkout's MCP server cannot be stopped")?;
        ensure!(
            started(&terminator) == Some(start),
            "MCP server process changed during inspection"
        );
        selected.insert(
            process.pid,
            OwnedProcess {
                handle: terminator,
                start,
                config: config_argument(args),
            },
        );
    }
    while !selected.is_empty() {
        let previous = selected.len();
        for process in &processes {
            if selected.contains_key(&process.pid) || protected.contains(&process.pid) {
                continue;
            }
            let Some(parent) = selected.get(&process.parent) else {
                continue;
            };
            let Some(recorded) = process.start else {
                continue;
            };
            let Some(handle) = open(process.pid, query | PROCESS_TERMINATE) else {
                ensure!(
                    open(process.pid, query).is_none(),
                    "An MCP server child cannot be stopped"
                );
                continue;
            };
            let Some(start) =
                started(&handle).filter(|start| *start == recorded && *start >= parent.start)
            else {
                continue;
            };
            selected.insert(
                process.pid,
                OwnedProcess {
                    handle,
                    start,
                    config: None,
                },
            );
        }
        if selected.len() == previous {
            break;
        }
    }
    let mut selected: Vec<_> = selected.into_values().collect();
    selected.sort_unstable_by_key(|process| process.start);
    Ok(selected)
}

pub(in crate::account_manager::interactive) fn running(root: &Path) -> Result<bool> {
    Ok(!owned_processes(root, false)?.is_empty())
}

pub(in crate::account_manager::interactive) fn stop(root: &Path) -> Result<Option<StoppedService>> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stopped_any = false;
    let mut recorded_config = None;
    // Re-check after termination so a child spawned by a launcher between the
    // initial snapshot and TerminateProcess is also caught and terminated.
    for _ in 0..3 {
        let processes = owned_processes(root, true)?;
        if processes.is_empty() {
            return Ok(stopped_any.then_some(StoppedService {
                config: recorded_config,
            }));
        }
        stopped_any = true;
        if recorded_config.is_none() {
            recorded_config = processes.iter().find_map(|process| process.config.clone());
        }
        for process in &processes {
            let handle = process.handle.as_raw_handle();
            // SAFETY: the pinned process handle has synchronize and terminate rights.
            unsafe {
                if WaitForSingleObject(handle, 0) == WAIT_TIMEOUT
                    && TerminateProcess(handle, 1) == 0
                    && WaitForSingleObject(handle, 0) != WAIT_OBJECT_0
                {
                    bail!("An MCP server process could not be stopped");
                }
            }
        }
        for process in &processes {
            // SAFETY: the pinned process handle has synchronize rights and remains live through this call.
            let status = unsafe {
                WaitForSingleObject(
                    process.handle.as_raw_handle(),
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis() as u32,
                )
            };
            ensure!(
                status == WAIT_OBJECT_0,
                "The MCP server did not stop within 15 seconds"
            );
        }
    }
    ensure!(
        owned_processes(root, false)?.is_empty(),
        "The MCP server continued spawning child processes; checkout is not quiescent"
    );
    Ok(stopped_any.then_some(StoppedService {
        config: recorded_config,
    }))
}

pub(in crate::account_manager::interactive) fn start(
    root: &Path,
    env_file: &Path,
    loaded_env: &[String],
) -> Result<u32> {
    let mut command = Command::new("pwsh.exe");
    for name in loaded_env {
        command.env_remove(name);
    }
    // Force HTTP as before, but let the native server resolve the selected file's address.
    let child = command
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
        .arg(root.join("start-mcp.ps1"))
        .arg("--config")
        .arg(env_file)
        .current_dir(root)
        .env("MCP_TRANSPORT", "http")
        .env("TELEGRAM_MCP_BINARY", std::env::current_exe()?)
        .creation_flags(CREATE_NEW_CONSOLE)
        .show_window(0)
        .spawn()
        .context("Could not start the PowerShell 7 launcher")?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_process_identity_requires_an_exact_script_or_serve_image() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let shell = Path::new(r"C:\pwsh.exe");
        let native = root.join(r"target\release\telegram-mcp.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, b"").unwrap();
        let launcher_path = root.join("start-mcp.ps1");
        std::fs::write(&launcher_path, b"").unwrap();
        let launcher = launcher_path.to_str().unwrap();
        let inspect = |image: &Path, args: &[&str]| {
            let args: Vec<_> = args.iter().copied().map(OsString::from).collect();
            service_args(root, image, &args).map(config_argument)
        };
        let matches = |image: &Path, args: &[&str]| inspect(image, args).is_some();
        let config = r"C:\configs\privaté file.env";
        let expected = Some(Some(PathBuf::from(config)));
        assert_eq!(
            inspect(
                shell,
                &[
                    "pwsh",
                    "-NoProfile",
                    "-NonInteractive",
                    "-File",
                    launcher,
                    "--config",
                    config,
                ]
            ),
            expected
        );
        for args in [
            ["pwsh", "-Command", "script", "-File", launcher].as_slice(),
            &["pwsh", r"C:\other\worker.ps1", "-File", launcher],
            &[
                "pwsh",
                "-NoProfile",
                r"C:\other\worker.ps1",
                "-File",
                launcher,
            ],
            &["pwsh", "-CommandWithArgs", "script", "-File", launcher],
        ] {
            assert!(!matches(shell, args));
        }
        assert!(!matches(
            shell,
            &["pwsh", "-File", r"C:\foreign\start-mcp.ps1", launcher]
        ));
        assert_eq!(inspect(&native, &["telegram-mcp", "serve"]), Some(None));
        assert_eq!(
            inspect(&native, &["telegram-mcp", "--config", config, "serve"]),
            expected
        );
        assert_eq!(
            inspect(
                &native,
                &["telegram-mcp", "serve", &format!("--config={config}")]
            ),
            expected
        );
        assert!(!matches(&native, &["telegram-mcp", "login", "serve"]));
        assert!(!matches(
            &native,
            &["telegram-mcp", "--config", config, "login"]
        ));
        assert!(!matches(&native, &["telegram-mcp", "session-health"]));
        assert!(!matches(
            Path::new(r"C:\foreign\telegram-mcp.exe"),
            &["telegram-mcp", "serve"]
        ));
        // Deleting the launcher fails closed rather than matching via string fallback.
        std::fs::remove_file(&launcher_path).unwrap();
        assert!(!matches(shell, &["pwsh", "-File", launcher]));
    }
}
