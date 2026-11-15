use super::*;
/// A difference request implicitly acknowledges encrypted updates in grammers.
/// Never attach a legacy authorization to that update consumer while this
/// account's old encryption custody is waiting for its separate Rust backend.
pub fn check_deferred_custody(state: &State, label: &str) -> Result<()> {
    validate_label(label)?;
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    if native["authorization_origin"].as_str() == Some("native") {
        return Ok(());
    }
    let directory = state.root().join("secret-chats");
    match directory.symlink_metadata() {
        Ok(metadata) => {
            // Never follow a redirected parent to decide that custody is
            // absent: a dangling link would turn child metadata into ENOENT.
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!(
                    "Cannot verify deferred secret-chat custody; its directory is not a regular owned directory"
                );
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    bail!("Cannot verify deferred secret-chat custody through a reparse point");
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => bail!(
            "Cannot verify deferred secret-chat custody; the legacy authorization was not connected"
        ),
    }
    for path in [
        directory.join(label),
        directory.join(format!("{label}.owner.json")),
    ] {
        match path.symlink_metadata() {
            Ok(_) => bail!(
                "This account has existing secret-chat custody. Keep its legacy authorization untouched until the separate Rust secret backend is ready, or log in with a fresh native authorization under a separate label."
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => bail!(
                "Cannot verify deferred secret-chat custody; the legacy authorization was not connected"
            ),
        }
    }
    Ok(())
}
