use super::*;
use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    sync::mpsc,
    time::Duration,
};

#[test]
fn replacing_a_validated_download_with_a_fifo_cannot_block_its_anchored_open() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let state = State::at(temp.path().join("state"), vec![root.clone()]).unwrap();
    let root = root.canonicalize().unwrap();
    let path = root.join("replacement");
    std::fs::write(&path, b"original regular file").unwrap();
    state.resolve_file(&path, FileAccess::Read).unwrap();
    let directory = Directory::open(&state, &root).unwrap();
    std::fs::remove_file(&path).unwrap();
    let native = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: native is an owned NUL-terminated path in the test's private root.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        entered_tx.send(()).unwrap();
        let result = directory
            .open_file(&CString::new("replacement").unwrap())
            .map(|file| file.metadata().unwrap().is_file());
        let _ = result_tx.send(result);
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let completed = result_rx.recv_timeout(Duration::from_secs(1));
    if completed.is_err() {
        // Rescue the old blocking implementation so its regression fails
        // explicitly rather than leaving a blocked reader in the test runner.
        let _writer = File::options()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path);
        if result_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            worker.join().unwrap();
        }
        panic!("A FIFO replacement blocked the anchored download open");
    }
    assert!(
        !completed.unwrap().unwrap(),
        "The raced descriptor must identify the FIFO"
    );
    worker.join().unwrap();
    assert!(
        open_download(&state, &path).is_err(),
        "A FIFO cannot become a download capability"
    );
}
