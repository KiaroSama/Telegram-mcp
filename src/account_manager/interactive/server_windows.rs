//! Native Windows process discovery. Only a checkout launcher or its serve process can seed a stop.
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
        processes.push(Process {
            pid: entry.pid,
            parent: entry.parent,
            name: String::from_utf16_lossy(&entry.name[..end]).to_ascii_lowercase(),
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

fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    text.strip_prefix("\\\\?\\")
        .unwrap_or(&text)
        .to_ascii_lowercase()
}

fn is_service(root: &Path, image: &Path, args: &[OsString]) -> bool {
    let Some(name) = image
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    match name.as_str() {
        "pwsh.exe" | "powershell.exe" => args
            .windows(2)
            .skip(1)
            .find_map(
                |pair| match pair[0].to_string_lossy().to_ascii_lowercase().as_str() {
                    "-file" | "-f" => Some(
                        path_text(Path::new(&pair[1])) == path_text(&root.join("start-mcp.ps1")),
                    ),
                    "-command" | "-c" | "-encodedcommand" | "-enc" => Some(false),
                    _ => None,
                },
            )
            .unwrap_or(false),
        "telegram-mcp.exe" | "telegram-mcp" => {
            args.get(1).is_some_and(|argument| argument == "serve")
                && ["", "bin", "target/release", "target/debug"]
                    .into_iter()
                    .any(|dir| path_text(&root.join(dir).join(&name)) == path_text(image))
        }
        _ => false,
    }
}

struct OwnedProcess {
    handle: OwnedHandle,
    start: u64,
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
        if !image(&handle)
            .zip(command_args(&handle))
            .is_some_and(|(image, args)| is_service(root, &image, &args))
        {
            continue;
        }
        if !terminate {
            return Ok(vec![OwnedProcess { handle, start }]);
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
            let Some(handle) = open(process.pid, query | PROCESS_TERMINATE) else {
                ensure!(
                    open(process.pid, query).is_none(),
                    "An MCP server child cannot be stopped"
                );
                continue;
            };
            let Some(start) = started(&handle).filter(|start| *start >= parent.start) else {
                continue;
            };
            selected.insert(process.pid, OwnedProcess { handle, start });
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

pub(in crate::account_manager::interactive) fn stop(root: &Path) -> Result<bool> {
    let processes = owned_processes(root, true)?;
    if processes.is_empty() {
        return Ok(false);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
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
    for process in processes {
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
    Ok(true)
}

pub(in crate::account_manager::interactive) fn start(root: &Path, env_file: &Path) -> Result<u32> {
    let address = super::super::super::bind_address()?;
    let child = Command::new("pwsh.exe")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(root.join("start-mcp.ps1"))
        .arg("--config")
        .arg(env_file)
        .arg("--http")
        .arg(address.to_string())
        .current_dir(root)
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
        let root = Path::new(r"C:\fixture checkout");
        let shell = Path::new(r"C:\pwsh.exe");
        let native = root.join(r"target\release\telegram-mcp.exe");
        let launcher = r"C:\fixture checkout\start-mcp.ps1";
        let matches = |image: &Path, args: &[&str]| {
            is_service(
                root,
                image,
                &args.iter().copied().map(OsString::from).collect::<Vec<_>>(),
            )
        };
        assert!(matches(shell, &["pwsh", "-NoProfile", "-File", launcher]));
        assert!(!matches(
            shell,
            &["pwsh", "-Command", "script", "-File", launcher]
        ));
        assert!(!matches(
            shell,
            &["pwsh", "-File", r"C:\foreign\start-mcp.ps1", launcher]
        ));
        assert!(matches(&native, &["telegram-mcp", "serve"]));
        assert!(!matches(&native, &["telegram-mcp", "login", "serve"]));
        assert!(!matches(&native, &["telegram-mcp", "session-health"]));
        assert!(!matches(
            Path::new(r"C:\foreign\telegram-mcp.exe"),
            &["telegram-mcp", "serve"]
        ));
    }
}
