//! Terminal account management backed by the native configuration transactions.
mod console;
mod files;
mod server;

use anyhow::Result;
use console::Console;
use files::{Account, RunLog};
use serde_json::Value;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

use super::{read_config, rename_configured_account, session_health, session_validate};
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
                "1" => files::accounts(&self.env_file).map(|accounts| show_accounts(&accounts)),
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
        let accounts = files::accounts(&self.env_file)?;
        let Some(label) = self
            .console
            .label("Label for the new account (e.g. work, personal)")?
        else {
            return cancelled();
        };
        let existing = accounts.iter().find(|account| account.label == label);
        if let Some(account) = existing {
            println!(
                "An account labelled '{label}' already exists ({}).",
                account.key
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
            if files::accounts(&self.env_file)?
                .iter()
                .any(|account| account.label == label)
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
        if files::accounts(&self.env_file)?.len() > 1 {
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

    async fn login(&self, label: &str, existing: Option<&Account>, health: &Value) -> Result<()> {
        let service = if existing.is_some() {
            server::stop(&self.root)?
        } else {
            false
        };
        let result = crate::backend::login_transaction(
            label,
            &self.env_file,
            existing.map(|account| account.key.as_str()),
            health["config_digest"]
                .as_str()
                .or_else(|| existing.map(|account| account.digest.as_str())),
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

    async fn health_or_unknown(&self, account: Option<&Account>) -> Value {
        match account {
            Some(account) => session_health(&self.env_file, &account.key)
                .await
                .unwrap_or_else(|_| serde_json::json!({"status": "unknown"})),
            None => Value::Null,
        }
    }

    async fn health(&mut self) -> Result<()> {
        let accounts = files::accounts(&self.env_file)?;
        if accounts.is_empty() {
            println!("No accounts are configured.");
            return Ok(());
        }
        println!("Checking current Telegram authorization (no login prompts)...");
        let mut repair = Vec::new();
        for account in &accounts {
            let health = self.health_or_unknown(Some(account)).await;
            let status = health_status(&health);
            println!("  {:<20} {status}", account.label);
            if repairable(status) {
                repair.push((account, health));
            }
        }
        println!("Network/unknown/busy means unverified, not invalid. No credential was changed.");
        if repair.is_empty() {
            return Ok(());
        }
        println!("Accounts that need re-login:");
        for (index, (account, _)) in repair.iter().enumerate() {
            println!("  {}. {}", index + 1, account.label);
        }
        let Some(index) = self.console.number("Number to re-login", repair.len())? else {
            println!("Cancelled. Nothing was changed.");
            return Ok(());
        };
        let (account, health) = &repair[index];
        println!(
            "Log in to the same Telegram account. Old device-bound secret chats cannot be restored by a new login; their files will be kept."
        );
        let label = &account.label;
        if self.console.confirm(&format!("Re-login for '{label}'?"))? {
            self.login(&account.label, Some(account), health).await?;
            println!(
                "Replacement saved for '{}'. The server verifies it before activation.",
                account.label
            );
        }
        Ok(())
    }

    fn remove(&mut self) -> Result<()> {
        let accounts = files::accounts(&self.env_file)?;
        if accounts.is_empty() {
            println!("There is nothing to remove.");
            return Ok(());
        }
        show_accounts(&accounts);
        let Some(index) = self.console.number("Number to remove", accounts.len())? else {
            return cancelled();
        };
        let Account { label, key, .. } = &accounts[index];
        if accounts.len() == 1 {
            println!(
                "This is the only account. The server cannot start until another is configured."
            );
        }
        println!("This removes {key} from .env and archives its private records.");
        println!(
            "The Telegram session stays authorised. To revoke it, go to Telegram Settings > Devices."
        );
        if !self.console.confirm(&format!("Remove '{label}'?"))? {
            return cancelled();
        }
        let state = State::global()?;
        self.with_stopped_service(|| super::removal_report(&state, label, &self.env_file, key))?;
        self.log
            .record("INFO", "Removed an account; state archived privately");
        println!("Removed '{label}'.");
        self.report_backup();
        Ok(())
    }

    fn rename(&mut self) -> Result<()> {
        let accounts = files::accounts(&self.env_file)?;
        if accounts.is_empty() {
            println!("There is nothing to rename.");
            return Ok(());
        }
        show_accounts(&accounts);
        let Some(index) = self.console.number("Number to rename", accounts.len())? else {
            return cancelled();
        };
        let Account {
            label: from, key, ..
        } = &accounts[index];
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
        if accounts.iter().any(|account| account.label == to) {
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
            let moved = rename_configured_account(&state, from, &to, &self.env_file, key)?;
            let key_prefix = if key.starts_with("TELEGRAM_SESSION_NAME") {
                "TELEGRAM_SESSION_NAME"
            } else {
                "TELEGRAM_SESSION_STRING"
            };
            let new_key = files::key_for(key_prefix, &to);
            match files::rename_registry(&self.root.join("secrets.md"), key, &new_key, from, &to) {
                Ok(true) => println!("  secrets.md: '## {key}' is now '## {new_key}'"),
                Ok(false) => {}
                Err(_) => {
                    println!("The local secrets registry was not updated. Rename its '## {key}' heading by hand.");
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

fn show_accounts(accounts: &[Account]) {
    if accounts.is_empty() {
        println!(
            "No accounts are configured yet. Choose 'Add an account' to configure the first one."
        );
        return;
    }
    println!("Configured accounts ({}):", accounts.len());
    for (index, account) in accounts.iter().enumerate() {
        let default = if account.label == "default" {
            " (default configuration)"
        } else {
            ""
        };
        println!(
            "  {}. {:<16} {}{default}",
            index + 1,
            account.label,
            account.key
        );
    }
    println!("Configured does not mean authorized. Option 6 checks current session health.");
    if accounts.len() > 1 {
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
