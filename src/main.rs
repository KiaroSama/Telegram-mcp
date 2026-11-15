use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Native Rust Telegram MCP server")]
struct Cli {
    /// Read one explicit UTF-8 configuration file (process values take precedence).
    #[arg(long, global = true, env = "TELEGRAM_ENV_FILE")]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Connect a stdio client to the existing shared HTTP service.
    Bridge,
    /// Serve MCP over stdin/stdout, or loopback HTTP for shared clients.
    Serve {
        #[arg(long)]
        http: Option<std::net::SocketAddr>,
    },
    /// Authorize a Telegram account using an interactive terminal.
    Login {
        #[arg(long, default_value = "default")]
        account: String,
        #[arg(long, default_value = ".env")]
        env_file: std::path::PathBuf,
        #[arg(long)]
        replace_key: Option<String>,
        #[arg(long)]
        expected_user_id: Option<i64>,
        #[arg(long)]
        expected_config_digest: Option<String>,
        #[arg(long)]
        no_echo: bool,
    },
    /// Validate a legacy session read from stdin without connecting or saving it.
    SessionValidate,
    /// Check authorization through the existing shared service.
    SessionHealth {
        #[arg(long, default_value = ".env")]
        env_file: std::path::PathBuf,
        #[arg(long)]
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// Rename private account records as one recoverable local transaction.
    RenameAccount {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,
    },
    /// Report configured accounts without revealing credentials.
    Accounts {
        #[arg(long)]
        json: bool,
    },
    /// Approve one pending operation in a genuine human terminal.
    Approve { fingerprint: String },
    /// Report local service readiness.
    Health {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        account: Option<String>,
    },
    /// Print the directory holding private persistent state.
    StatePath,
}
fn main() -> Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|value| value == "__document-worker")
    {
        return telegram_mcp::documents::worker_entry();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|value| value == "__capture-worker")
    {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                telegram_mcp::tools::visual::capture::worker_entry().await?;
                Ok(())
            });
    }
    let mut cli = Cli::parse();
    let env_file = cli.config.clone().unwrap_or_else(|| match &cli.command {
        Some(Command::Login { env_file, .. }) | Some(Command::SessionHealth { env_file, .. }) => {
            env_file.clone()
        }
        _ => std::path::PathBuf::from(".env"),
    });
    match cli.command.as_mut() {
        Some(Command::Login {
            env_file: selected, ..
        })
        | Some(Command::SessionHealth {
            env_file: selected, ..
        }) => *selected = env_file.clone(),
        _ => {}
    }
    telegram_mcp::account_manager::select_source(&env_file)?;
    telegram_mcp::account_manager::capture_process_config();
    let snapshot = telegram_mcp::account_manager::read_config(&env_file)?;
    for (key, value) in snapshot.values {
        if std::env::var_os(&key).is_none() {
            // SAFETY: startup is synchronous. No worker, logging or network
            // threads exist until the Tokio runtime is constructed below.
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}
async fn run(cli: Cli) -> Result<()> {
    match cli.command.unwrap_or(Command::Serve { http: None }) {
        Command::Bridge => telegram_mcp::server::bridge::run().await?,
        Command::StatePath => {
            println!("{}", telegram_mcp::state::State::global()?.root().display())
        }
        Command::Accounts { .. } => println!("{}", telegram_mcp::backend::accounts_snapshot()?),
        Command::Login {
            account,
            env_file,
            replace_key,
            expected_user_id,
            expected_config_digest,
            ..
        } => {
            let result = telegram_mcp::backend::login_transaction(
                &account,
                &env_file,
                replace_key.as_deref(),
                expected_config_digest.as_deref(),
                expected_user_id,
            )
            .await?;
            println!("{result}");
        }
        Command::SessionValidate => {
            use std::io::Read;
            let mut value = String::new();
            std::io::stdin().take(4097).read_to_string(&mut value)?;
            println!(
                "{}",
                if value.len() > 4096 {
                    "invalid"
                } else {
                    telegram_mcp::account_manager::session_validate(&value)
                }
            );
        }
        Command::SessionHealth { env_file, key, .. } => println!(
            "{}",
            telegram_mcp::account_manager::session_health(&env_file, &key).await?
        ),
        Command::RenameAccount {
            from,
            to,
            state_dir,
        } => {
            let state = if let Some(root) = state_dir {
                std::sync::Arc::new(telegram_mcp::state::State::at(root, Vec::new())?)
            } else {
                telegram_mcp::state::State::global()?
            };
            let moved = telegram_mcp::account_manager::rename_account(&state, &from, &to)?;
            println!(
                "{}",
                serde_json::json!({"renamed":true,"from":from,"to":to,"moved":moved})
            );
        }
        Command::Approve { fingerprint } => telegram_mcp::safeguard::approve_from_terminal(
            telegram_mcp::state::State::global()?,
            &fingerprint,
        )?,
        Command::Health { account, .. } => {
            println!("{}", telegram_mcp::backend::health(account.as_deref())?)
        }
        Command::Serve { mut http } => {
            if http.is_none() {
                match std::env::var("MCP_TRANSPORT")
                    .unwrap_or_else(|_| "stdio".into())
                    .as_str()
                {
                    "stdio" => {}
                    "http" | "streamable-http" => {
                        let host = std::env::var("MCP_HOST").unwrap_or_else(|_| "127.0.0.1".into());
                        let port = std::env::var("MCP_PORT")
                            .unwrap_or_else(|_| "8765".into())
                            .parse::<u16>()?;
                        http = Some(std::net::SocketAddr::new(host.parse()?, port));
                    }
                    _ => anyhow::bail!("MCP_TRANSPORT must be stdio or http"),
                }
            }
            telegram_mcp::server::serve(http).await?;
        }
    }
    Ok(())
}
