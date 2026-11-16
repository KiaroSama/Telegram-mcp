//! Darwin's address-space floor can exceed the entire parser budget at launch.
//! Monitor the worker's own physical footprint and lifetime high-water mark.
//! No PID is accepted from input, and the watchdog can terminate only itself.
//! Native fields: apple-oss-distributions/xnu bsd/sys/resource.h (RUSAGE_INFO_V4),
//! populated by bsd/kern/kern_resource.c::gather_rusage_info.
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

const MEMORY_EXIT: i32 = 78;
type Sampler = Arc<dyn Fn() -> io::Result<u64> + Send + Sync>;

pub(super) struct MemoryGuard {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    sample: Sampler,
    budget: u64,
}
impl MemoryGuard {
    pub(super) fn start(
        budget: u64,
        interval: Duration,
        sample: impl Fn() -> io::Result<u64> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        if budget == 0 || interval.is_zero() {
            return Err(io::Error::other(
                "Invalid document memory supervisor budget",
            ));
        }
        let sample: Sampler = Arc::new(sample);
        check(&sample, budget)?;
        let (stop, stopped) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let probe = Arc::clone(&sample);
        let thread = thread::Builder::new()
            .name("pdf-memory-budget".into())
            .spawn(move || {
                // Catch probe panics too: a dead supervisor must never leave
                // an already-started parser running without memory accounting.
                let outcome = catch_unwind(AssertUnwindSafe(|| -> io::Result<()> {
                    let initial = check(&probe, budget);
                    let allowed = initial.is_ok();
                    if ready.send(initial).is_err() || !allowed {
                        return Ok(());
                    }
                    loop {
                        match stopped.recv_timeout(interval) {
                            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                            Err(mpsc::RecvTimeoutError::Timeout) => check(&probe, budget)?,
                        }
                    }
                }));
                if !matches!(outcome, Ok(Ok(()))) {
                    // Exit this dedicated parser only. There is no caller-
                    // supplied PID and no opportunity for PID reuse.
                    std::process::exit(MEMORY_EXIT);
                }
            })?;
        let mut guard = Self {
            stop: Some(stop),
            thread: Some(thread),
            sample,
            budget,
        };
        match started.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(guard),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                guard.stop_and_join()?;
                Err(io::Error::other("Document memory supervisor did not start"))
            }
        }
    }

    pub(super) fn finish(mut self) -> io::Result<()> {
        // Lifetime peak catches allocations freed between scheduled probes.
        let result = check(&self.sample, self.budget);
        self.stop_and_join()?;
        result
    }

    fn stop_and_join(&mut self) -> io::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| io::Error::other("Document memory supervisor failed"))?;
        }
        Ok(())
    }
}
impl Drop for MemoryGuard {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

fn check(sample: &Sampler, budget: u64) -> io::Result<()> {
    if sample()? > budget {
        Err(io::Error::other(
            "Document worker exceeded its physical-memory budget",
        ))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(super) fn physical_footprint() -> io::Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: query only this process, with an exactly sized V4 output buffer.
    // proc_pid_rusage writes the struct itself (its C ABI calls it void **).
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            usage.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the native call succeeded and initialized the V4 structure.
    let usage = unsafe { usage.assume_init() };
    if usage.ri_phys_footprint == 0 {
        return Err(io::Error::other(
            "Document memory accounting is unavailable",
        ));
    }
    Ok(usage
        .ri_phys_footprint
        .max(usage.ri_lifetime_max_phys_footprint))
}

#[cfg(test)]
#[path = "../../../tests/unit/document_memory.rs"]
mod tests;
