use std::sync::Arc;
use telegram_mcp::state::State;
pub fn fixture() -> (tempfile::TempDir, Arc<State>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("installation/files")).unwrap();
    std::fs::create_dir_all(dir.path().join("allowed")).unwrap();
    let state = State::at_with_installation(
        dir.path().join("private"),
        vec![dir.path().join("allowed")],
        dir.path().join("installation"),
    )
    .unwrap();
    (dir, Arc::new(state))
}
