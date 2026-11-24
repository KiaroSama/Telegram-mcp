use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    if telegram_mcp::tools::visual::capture::worker_entry().await? {
        return Ok(());
    }
    telegram_mcp::emoji::cli::run().await
}
