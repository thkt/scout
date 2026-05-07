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

use std::io::{self, ErrorKind, Write, stderr, stdout};
use std::process::ExitCode;

use clap::Parser;
use tools::{Command, Scout};

fn write_output<W: Write>(w: &mut W, output: &str) -> io::Result<()> {
    w.write_all(output.as_bytes())?;
    if !output.ends_with('\n') {
        w.write_all(b"\n")?;
    }
    Ok(())
}

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
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

pub async fn run() -> ExitCode {
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
            return ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(1_u8));
        }
    };

    match scout.run(cli.command).await {
        Ok(output) => {
            let mut handle = stdout().lock();
            match write_output(&mut handle, &output) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(1_u8))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use clap::CommandFactory;

    use super::write_output;

    /// [T-W001] write_output appends newline when output lacks trailing newline
    #[test]
    fn t_w001_write_output_appends_newline_when_missing() {
        let mut buf = Vec::new();
        write_output(&mut buf, "hello").unwrap();
        assert_eq!(&buf, b"hello\n");
    }

    /// [T-W002] write_output preserves single trailing newline
    #[test]
    fn t_w002_write_output_preserves_existing_newline() {
        let mut buf = Vec::new();
        write_output(&mut buf, "hello\n").unwrap();
        assert_eq!(&buf, b"hello\n");
    }

    /// [T-W003] write_output propagates BrokenPipe error from writer
    #[test]
    fn t_w003_write_output_propagates_broken_pipe() {
        struct BrokenPipeWriter;
        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut w = BrokenPipeWriter;
        let err = write_output(&mut w, "hello").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

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
