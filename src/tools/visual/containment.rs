use anyhow::{Context, Result};
use tokio::process::{Child, Command};

#[cfg(windows)]
pub(crate) struct Tree(std::os::windows::io::OwnedHandle);
#[cfg(unix)]
pub(crate) struct Tree(i32);
#[cfg(unix)]
impl Drop for Tree {
    fn drop(&mut self) {
        // SAFETY: the positive ID was assigned to the private group by process_group(0).
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}
#[cfg(unix)]
pub(crate) fn spawn(cmd: &mut Command) -> Result<(Child, Tree)> {
    let child = cmd.process_group(0).kill_on_drop(true).spawn()?;
    let id = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 0)
        .context("Missing worker group ID")?;
    Ok((child, Tree(id)))
}
#[cfg(windows)]
pub(crate) fn spawn(cmd: &mut Command) -> Result<(Child, Tree)> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        System::{JobObjects::*, Threading::CREATE_SUSPENDED},
    };
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }
    // SAFETY: null attributes and name request an unnamed job with no inherited handle.
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    anyhow::ensure!(
        !handle.is_null(),
        "Cannot create worker job: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: CreateJobObjectW returned a new, valid, uniquely owned handle.
    let job = Tree(unsafe { OwnedHandle::from_raw_handle(handle) });
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: info is the correct fixed-size structure and lives through this call.
    let ok = unsafe {
        SetInformationJobObject(
            job.0.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    anyhow::ensure!(
        ok != 0,
        "Cannot configure worker job: {}",
        std::io::Error::last_os_error()
    );
    let child = cmd
        .creation_flags(CREATE_SUSPENDED)
        .kill_on_drop(true)
        .spawn()?;
    let process = child
        .raw_handle()
        .context("Missing worker process handle")?;
    // SAFETY: both handles remain live; the new worker has not run or spawned any descendants.
    let assigned = unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), process) };
    anyhow::ensure!(
        assigned != 0,
        "Cannot contain worker process: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: process belongs to the child created suspended above, now in the owned job.
    anyhow::ensure!(
        unsafe { NtResumeProcess(process) } >= 0,
        "Cannot resume worker process"
    );
    Ok((child, job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    const FIXTURE: &str = "tools::visual::capture::containment::tests::descendant_fixture";
    const FLAG: &str = "PARITY_CAPTURE_TREE_FIXTURE";
    #[test]
    fn descendant_fixture() {
        match std::env::var(FLAG).as_deref() {
            Ok("parent") => {
                let mut leaf = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", FIXTURE, "--nocapture"])
                    .env(FLAG, "leaf")
                    .spawn()
                    .unwrap();
                leaf.wait().unwrap();
            }
            Ok("leaf") => {
                println!("TREE_READY");
                std::thread::sleep(Duration::from_secs(20));
            }
            _ => {}
        }
    }
    #[tokio::test]
    async fn dropping_tree_closes_descendants_inherited_pipe() {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", FIXTURE, "--nocapture"])
            .env(FLAG, "parent")
            .stdout(Stdio::piped());
        let (mut child, tree) = spawn(&mut cmd).unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                line.clear();
                assert_ne!(out.read_line(&mut line).await.unwrap(), 0);
                if line.contains("TREE_READY") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(tree);
        tokio::time::timeout(Duration::from_secs(3), async {
            child.wait().await.unwrap();
            out.read_to_end(&mut vec![]).await.unwrap();
        })
        .await
        .expect("a descendant kept the capture pipe open");
    }
}
