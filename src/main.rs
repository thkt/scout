mod fetch;
mod gemini;
mod github;
mod search;
mod tools;

pub const USER_AGENT: &str = concat!("scout/", env!("CARGO_PKG_VERSION"), " (MCP Server)");

use rmcp::{ServiceExt, transport::stdio};
use tools::Scout;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("scout=info".parse()?),
        )
        .init();

    info!("starting scout MCP server");

    let service = Scout::new()?
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("failed to start server: {e}"))?;

    service.waiting().await?;
    info!("server stopped");
    Ok(())
}
