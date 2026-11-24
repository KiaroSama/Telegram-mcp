//! Account publication, legacy configuration and cross-writer regressions.
mod local_support;
use local_support::fixture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::{
    account_manager::{
        begin_login, config_digest, parse_config, read_config, recover_logins, recover_rename,
        remove_account, rename_configured_account, write_env_value,
    },
    state::State,
};

fn snapshot(user: i64, key: u8) -> Value {
    let mut data = grammers_session::SessionData::default();
    data.dc_options.get_mut(&data.home_dc).unwrap().auth_key = Some([key; 256]);
    json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[grammers_session::types::PeerInfo::User{id:user,auth:None,bot:Some(false),is_self:Some(true)}],"updates_state":data.updates_state})
}
fn login_marker(state: &State, env: &std::path::Path, installed: Value) -> String {
    let stage = "__login_recovery_fixture";
    let mut staged = installed.clone();
    staged["authorization_source"] = Value::Null;
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    let name = "account-login-work-transaction.json";
    state.write_json(name,&json!({"account":"work","stage":stage,"stage_digest":config_digest(&serde_json::to_string(&staged).unwrap()),"env_file":env,"config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work"),"installed":installed})).unwrap();
    name.to_owned()
}
fn bound() -> Value {
    let mut value = snapshot(42, 7);
    value["authorization_source"] = json!(format!(
        "TELEGRAM_SESSION_NAME_WORK:{}",
        config_digest("work")
    ));
    value
}

fn completed_login_fixture(state: &State, stage: &str, staged: &Value) -> Value {
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        state.root().as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::OsStrExt;
        state
            .root()
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    json!({"version":2,"phase":"completed","owner":format!("{:x}",Sha256::digest(path)),"account":"work","stage":stage,"stage_digest":config_digest(&serde_json::to_string(staged).unwrap()),"committed":true})
}

// Keep the fully qualified names used by the CI test inventory.
include!("rust_account_review/login.rs");
include!("rust_account_review/configuration.rs");
include!("rust_account_review/removal.rs");
include!("rust_account_review/rename.rs");
