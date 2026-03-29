mod fetch;
mod gemini;
mod github;
mod markdown;
mod redacted;
mod retry;
mod search;
mod slack;
#[cfg(test)]
mod test_support;
mod tools;

pub const USER_AGENT: &str = concat!("scout/", env!("CARGO_PKG_VERSION"));

use clap::Parser;
use tools::{Command, Scout};

#[derive(Parser)]
#[command(
    name = "scout",
    version,
    about = "Web search, page fetching, and GitHub repository exploration"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                "scout=info".parse().unwrap_or_else(|_| {
                    tracing_subscriber::filter::Directive::from(tracing::Level::INFO)
                }),
            ),
        )
        .init();

    let cli = Cli::parse();

    let scout = match Scout::new().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(e.exit_code());
        }
    };

    match scout.run(cli.command).await {
        Ok(output) => {
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}
