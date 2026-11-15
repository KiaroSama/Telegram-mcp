//! Guidance-only plugin distribution. No client registration or Telegram login occurs.
#[path = "operator/install.rs"]
mod install;
#[path = "operator/package.rs"]
mod package;
#[path = "operator/payload.rs"]
mod payload;
#[cfg(test)]
#[path = "../../tests/unit/operator_plugin.rs"]
mod tests;

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Install or package the canonical Telegram operator skills"
)]
struct Cli {
    /// Repository containing the canonical skill payload, not private client configuration.
    #[arg(long, default_value = ".", global = true)]
    source: PathBuf,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Copy five complete skills to an explicitly chosen project/client or skills directory.
    Install {
        #[arg(long, requires = "project")]
        client: Option<String>,
        #[arg(long, conflicts_with = "skills_dir", requires = "client")]
        project: Option<PathBuf>,
        #[arg(long, conflicts_with = "project")]
        skills_dir: Option<PathBuf>,
    },
    /// Create an allowlisted guidance-only portable or Antigravity ZIP archive.
    Package {
        #[arg(long, value_parser = ["portable", "antigravity"])]
        client: String,
        #[arg(long)]
        output: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    if telegram_mcp::logging::initialize().is_err() {
        eprintln!("Private operator-helper logging could not be initialized.");
        std::process::exit(1);
    }
    let action = match &cli.command {
        Action::Install { .. } => "install",
        Action::Package { .. } => "package",
    };
    if let Err(error) = execute(cli) {
        tracing::error!(
            action,
            "Operator helper refused or failed; existing files were not overwritten"
        );
        eprintln!("Operator {action} failed: {error}");
        std::process::exit(1);
    }
    tracing::info!(action, "Operator guidance operation completed");
}

fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Action::Install {
            client,
            project,
            skills_dir,
        } => {
            ensure!(
                project.is_some() || skills_dir.is_some(),
                "Choose --project with --client, or --skills-dir"
            );
            let destination = if let Some(directory) = skills_dir {
                directory
            } else {
                let route = payload::client_directory(client.as_deref().unwrap_or_default())?;
                project.expect("Project validated").join(route)
            };
            install::install(&cli.source, &destination)?;
            println!(
                "Installed five complete skills. Restart the client and verify discovery; MCP setup is separate."
            );
        }
        Action::Package { client, output } => {
            package::package(&cli.source, &output, &client)?;
            println!("Created the {client} guidance-only plugin archive.");
        }
    }
    Ok(())
}
