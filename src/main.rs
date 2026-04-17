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

pub(crate) const USER_AGENT: &str = concat!("scout/", env!("CARGO_PKG_VERSION"));

use std::io::stderr;
use std::process::exit;

use clap::Parser;
use tools::{Command, Scout};

#[derive(Parser)]
#[command(
    name = "scout",
    version,
    about = "Web search, page fetching, and GitHub repository exploration",
    after_help = "\
Exit codes:
  0  Success
  1  User error (invalid input, not found, auth failure)
  2  Internal error or transient network failure

Environment:
  GEMINI_API_KEY  Required for search and research commands
  GITHUB_TOKEN    Optional for GitHub commands (higher rate limits)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() {
    use tracing_subscriber::filter::Directive;
    tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                "scout=info"
                    .parse()
                    .unwrap_or_else(|_| Directive::from(tracing::Level::INFO)),
            ),
        )
        .init();

    let cli = Cli::parse();

    let scout = match Scout::new().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            exit(e.exit_code());
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
            exit(e.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// [T-H000] root --help contains Exit codes: and Environment: sections
    #[test]
    fn t_h000_root_help_contains_exit_codes_and_environment() {
        let help = super::Cli::command().render_long_help().to_string();
        assert!(
            help.contains("Exit codes:"),
            "root help missing Exit codes:"
        );
        assert!(
            help.contains("GEMINI_API_KEY"),
            "root help missing GEMINI_API_KEY"
        );
        assert!(
            help.contains("GITHUB_TOKEN"),
            "root help missing GITHUB_TOKEN"
        );
        assert!(
            help.contains("User error"),
            "root help missing exit code 1 description"
        );
        assert!(
            help.contains("transient network failure"),
            "root help missing exit code 2 description"
        );
    }
}
