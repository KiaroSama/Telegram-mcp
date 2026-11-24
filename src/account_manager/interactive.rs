//! Terminal account management backed by the native configuration transactions.
mod console;
mod files;
mod server;

use anyhow::Result;
use console::Console;
use files::{ConfiguredProfile, RunLog};
use serde_json::Value;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

use super::{read_config, session_health, session_validate};
use crate::state::State;

/// Manage the accounts owned by the selected dotenv file.
pub async fn run(env_file: &Path) -> Result<()> {
    let root = std::env::current_dir()?;
    let mut manager = Manager {
        console: Console::new(),
        log: RunLog::open(&root),
        env_file: root.join(env_file),
        root,
    };
    manager.log.record("INFO", "Account manager started");
    let result = async {
        State::global()?;
        manager.menu().await
    }
    .await;
    manager.log.record(
        if result.is_ok() { "INFO" } else { "ERROR" },
        if result.is_ok() {
            "Account manager stopped with exit code 0"
        } else {
            "Account manager stopped with exit code 1"
        },
    );
    result
}

struct Manager {
    console: Console,
    log: RunLog,
    root: PathBuf,
    env_file: PathBuf,
}

impl Manager {
    async fn menu(&mut self) -> Result<()> {
        if !self.env_file.is_file() {
            println!("No .env file exists at {}.", self.env_file.display());
            println!(
                "Copy .env.example and set TELEGRAM_API_ID and TELEGRAM_API_HASH before logging in."
            );
            if !self
                .console
                .confirm("Create an empty .env now so accounts can be added?")?
            {
                println!("Nothing was changed.");
                return Ok(());
            }
            files::create_config(&self.env_file)?;
            self.log.record("INFO", "Created an empty .env");
        }
        while !self.console.quitting {
            self.console.menu();
            let Some(choice) = self.console.answer("Selection", false)? else {
                continue;
            };
            let result = match choice.as_str() {
                "1" => files::configured_profiles(&self.env_file)
                    .map(|profiles| show_profiles(&profiles)),
                "2" => self.add().await,
                "3" => self.remove(),
                "4" => self.rename(),
                "5" => self.new_login().await,
                "6" => self.health().await,
                _ => {
                    println!("Enter a menu number from 1 to 6, or exit.");
                    continue;
                }
            };
            if let Err(error) = result {
                println!("Failed: {error}");
                self.log.record("ERROR", "Account operation failed");
            }
        }
        Ok(())
    }

    async fn add(&mut self) -> Result<()> {
        let profiles = files::configured_profiles(&self.env_file)?;
        let Some(label) = self
            .console
            .label("Label for the new account (e.g. work, personal)")?
        else {
            return cancelled();
        };
        let existing = profiles.iter().find(|profile| profile.label == label);
        if let Some(profile) = existing {
            println!(
                "An account labelled '{label}' already exists ({}).",
                profile.env_name
            );
            if !self.console.confirm("Replace its session?")? {
                return cancelled();
            }
        }
        println!("A session string authorises full access to that Telegram account.");
        if self
            .console
            .confirm("Log in and save native authorization now?")?
        {
            let health = self.health_or_unknown(existing).await;
            self.login(&label, existing, &health).await?;
            if files::configured_profiles(&self.env_file)?
                .iter()
                .any(|profile| profile.label == label)
            {
                println!("The generator saved '{label}' to .env.");
                println!("A running server picks this up on its own.");
            }
            return Ok(());
        }
        if self.console.quitting {
            return Ok(());
        }
        let session = Zeroizing::new(
            self.console
                .secret("Paste the session string (input stays hidden)")?,
        );
        if session.trim().is_empty() {
            println!("Nothing was pasted; no change made.");
            return Ok(());
        }
        if session.len() > 4096 || session_validate(&session) != "valid" {
            println!(
                "The native parser cannot read that legacy session string. Nothing was saved."
            );
            return Ok(());
        }
        files::import_session(&self.env_file, &label, existing, session.trim())?;
        println!("Added '{label}'.");
        self.log.record("INFO", "Saved an imported account");
        self.report_backup();
        println!("A running server picks this up on its own.");
        if files::configured_profiles(&self.env_file)?.len() > 1 {
            println!("Name the account on tool calls, for example account={label}.");
        }
        Ok(())
    }

    async fn new_login(&mut self) -> Result<()> {
        if let Some(label) = self.console.label("Label for the account to log in")? {
            println!("Log in as the account you want to add.");
            if self.console.confirm("Run the session generator now?")? {
                self.login(&label, None, &Value::Null).await?;
            }
        }
        Ok(())
    }

    async fn login(
        &self,
        label: &str,
        existing: Option<&ConfiguredProfile>,
        health: &Value,
    ) -> Result<()> {
        let service = if existing.is_some() {
            server::stop(&self.root)?
        } else {
            false
        };
        let result = crate::backend::login_transaction(
            label,
            &self.env_file,
            existing.map(|profile| profile.env_name.as_str()),
            health["config_digest"]
                .as_str()
                .or_else(|| existing.map(|profile| profile.digest.as_str())),
            health["user_id"].as_i64().filter(|id| *id > 0),
        )
        .await;
        self.restore_service(service);
        match result {
            Ok(_) => {
                self.log.record("INFO", "Saved native authorization");
                println!("Saved native authorization for '{label}'.");
                Ok(())
            }
            Err(error) => Err(error.context(
                "The generator did not save a replacement. Previous configuration was not changed",
            )),
        }
    }

    async fn health_or_unknown(&self, profile: Option<&ConfiguredProfile>) -> Value {
        match profile {
            Some(profile) => session_health(&self.env_file, &profile.env_name)
                .await
                .unwrap_or_else(|_| serde_json::json!({"status": "unknown"})),
            None => Value::Null,
        }
    }

    async fn health(&mut self) -> Result<()> {
        let profiles = files::configured_profiles(&self.env_file)?;
        if profiles.is_empty() {
            println!("No accounts are configured.");
            return Ok(());
        }
        println!("Checking current Telegram authorization (no login prompts)...");
        let mut repair = Vec::new();
        for profile in &profiles {
            let health = self.health_or_unknown(Some(profile)).await;
            let status = health_status(&health);
            println!("  {:<20} {status}", profile.label);
            if repairable(status) {
                repair.push((profile, health));
            }
        }
        println!("Network/unknown/busy means unverified, not invalid. No credential was changed.");
        if repair.is_empty() {
            return Ok(());
        }
        println!("Accounts that need re-login:");
        for (index, (profile, _)) in repair.iter().enumerate() {
            println!("  {}. {}", index + 1, profile.label);
        }
        let Some(index) = self.console.number("Number to re-login", repair.len())? else {
            println!("Cancelled. Nothing was changed.");
            return Ok(());
        };
        let (profile, health) = &repair[index];
        println!(
            "Log in to the same Telegram account. Old device-bound secret chats cannot be restored by a new login; their files will be kept."
        );
        let label = &profile.label;
        if self.console.confirm(&format!("Re-login for '{label}'?"))? {
            self.login(&profile.label, Some(profile), health).await?;
            println!(
                "Replacement saved for '{}'. The server verifies it before activation.",
                profile.label
            );
        }
        Ok(())
    }

    fn remove(&mut self) -> Result<()> {
        let profiles = files::configured_profiles(&self.env_file)?;
        if profiles.is_empty() {
            println!("There is nothing to remove.");
            return Ok(());
        }
        show_profiles(&profiles);
        let Some(index) = self.console.number("Number to remove", profiles.len())? else {
            return cancelled();
        };
        let ConfiguredProfile {
            label, env_name, ..
        } = &profiles[index];
        if profiles.len() == 1 {
            println!(
                "This is the only account. The server cannot start until another is configured."
            );
        }
        println!("This removes {env_name} from .env and archives its private records.");
        println!(
            "The Telegram session stays authorised. To revoke it, go to Telegram Settings > Devices."
        );
        if !self.console.confirm(&format!("Remove '{label}'?"))? {
            return cancelled();
        }
        let state = State::global()?;
        self.with_stopped_service(|| {
            super::removal_report(&state, label, &self.env_file, env_name)
        })?;
        self.log
            .record("INFO", "Removed an account; state archived privately");
        println!("Removed '{label}'.");
        self.report_backup();
        Ok(())
    }

    fn rename(&mut self) -> Result<()> {
        let profiles = files::configured_profiles(&self.env_file)?;
        if profiles.is_empty() {
            println!("There is nothing to rename.");
            return Ok(());
        }
        show_profiles(&profiles);
        let Some(index) = self.console.number("Number to rename", profiles.len())? else {
            return cancelled();
        };
        let ConfiguredProfile {
            label: from,
            env_name,
            ..
        } = &profiles[index];
        if from == "default" {
            println!(
                "The default account cannot be renamed here. Remove it and add it back under a label."
            );
            return Ok(());
        }
        let Some(to) = self.console.label(&format!("New label for '{from}'"))? else {
            return cancelled();
        };
        if *from == to {
            println!("That is already its label.");
            return Ok(());
        }
        if profiles.iter().any(|profile| profile.label == to) {
            println!("'{to}' is already taken.");
            return Ok(());
        }
        println!(
            "This renames '{from}' to '{to}' in .env and the local secrets registry and moves its stored state. It does not change the Telegram account."
        );
        if server::running(&self.root)? {
            println!("The running MCP server will stop for the rename and start again afterwards.");
        }
        if !self
            .console
            .confirm(&format!("Rename '{from}' to '{to}'?"))?
        {
            return cancelled();
        }
        let state = State::global()?;
        let moved = self.with_stopped_service(|| {
            let moved = files::rename_profile_files(&state, &self.env_file, env_name, from, &to)?;
            let prefix = if env_name.starts_with("TELEGRAM_SESSION_NAME") {
                "TELEGRAM_SESSION_NAME"
            } else {
                "TELEGRAM_SESSION_STRING"
            };
            let new_env_name = files::env_name_for(prefix, &to);
            match files::rename_registry(
                &self.root.join("secrets.md"),
                env_name,
                &new_env_name,
                from,
                &to,
            ) {
                Ok(true) => println!("  secrets.md: '## {env_name}' is now '## {new_env_name}'"),
                Ok(false) => {}
                Err(_) => {
                    println!("The local secrets registry was not updated. Rename its '## {env_name}' heading by hand.");
                    self.log.record("WARNING", "The local secrets registry was not updated");
                }
            }
            Ok(moved)
        })?;
        self.log.record("INFO", "Renamed an account");
        println!("Renamed '{from}' to '{to}'.");
        for item in moved {
            println!("  moved: {item}");
        }
        self.report_backup();
        println!("Tools now take account={to}.");
        Ok(())
    }

    fn with_stopped_service<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let stopped = server::stop(&self.root)?;
        let result = operation();
        self.restore_service(stopped);
        result
    }

    fn restore_service(&self, stopped: bool) {
        if !stopped {
            return;
        }
        match server::start(&self.root, &self.env_file) {
            Ok(pid) => {
                self.log.record("INFO", "Restarted the MCP server");
                println!("MCP server started again in the background (pid {pid}).");
            }
            Err(error) => {
                self.log
                    .record("WARNING", "Could not restart the MCP server");
                eprintln!("Could not restart the MCP server: {error}");
            }
        }
    }

    fn report_backup(&self) {
        if let Some(name) = files::latest_backup(&self.env_file) {
            println!("Previous .env kept as {}", name.to_string_lossy());
        }
    }
}

fn cancelled() -> Result<()> {
    println!("Cancelled.");
    Ok(())
}

fn show_profiles(profiles: &[ConfiguredProfile]) {
    if profiles.is_empty() {
        println!(
            "No accounts are configured yet. Choose 'Add an account' to configure the first one."
        );
        return;
    }
    println!("Configured accounts ({}):", profiles.len());
    for (index, profile) in profiles.iter().enumerate() {
        let default = if profile.label == "default" {
            " (default configuration)"
        } else {
            ""
        };
        println!(
            "  {}. {:<16} {}{default}",
            index + 1,
            profile.label,
            profile.env_name
        );
    }
    println!("Configured does not mean authorized. Option 6 checks current session health.");
    if profiles.len() > 1 {
        println!("Name the account on tool calls with account=<label>.");
    }
}

fn health_status(health: &Value) -> &str {
    match health["status"].as_str() {
        Some(
            status @ ("healthy" | "unauthorized" | "revoked" | "invalid" | "busy" | "network"),
        ) => status,
        _ => "unknown",
    }
}

fn repairable(status: &str) -> bool {
    matches!(status, "unauthorized" | "revoked" | "invalid")
}

/// Fall back to the checkout dotenv for state when a launcher selects another file.
pub fn checkout_state_dir(selected: &Path) -> Result<Option<String>> {
    if selected == Path::new(".env") {
        return Ok(None);
    }
    Ok(read_config(Path::new(".env"))?
        .values
        .remove("TELEGRAM_STATE_DIR"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_alone_controls_repair_eligibility() {
        for status in ["healthy", "busy", "network", "unknown", "not-a-status"] {
            assert!(!repairable(health_status(
                &serde_json::json!({"status": status, "repairable": true})
            )));
        }
        for status in ["invalid", "revoked", "unauthorized"] {
            assert!(repairable(health_status(
                &serde_json::json!({"status": status, "repairable": false})
            )));
        }
    }
}
