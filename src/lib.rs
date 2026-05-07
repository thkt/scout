mod envelope;
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

use std::env;
use std::io::{self, ErrorKind, Write, stderr, stdout};
use std::process::ExitCode;

use clap::Parser;
use envelope::{CommandOutput, ErrorCode, ErrorEnvelope, ErrorPayload, SuccessEnvelope};
use tools::{Command, Scout, ScoutError};

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
Exit codes (sysexits.h):
  0   Success
  64  Usage error (clap parse, missing API key, conflicts_with violation)
  65  Data error (invalid input, malformed format, encoding error)
  66  Not found (repo/file not found, 404)
  74  IO error (network IO, write failure other than BrokenPipe)
  75  Temporary failure (rate limit, 5xx, retryable)

Environment:
  GEMINI_API_KEY  Required for search and research commands
  GITHUB_TOKEN    Optional for GitHub commands (higher rate limits)"
)]
pub(crate) struct Cli {
    /// Emit output as a JSON envelope (one line) on stdout
    /// instead of Markdown. Errors print a JSON envelope on stderr.
    #[arg(long, global = true)]
    json: bool,

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

    // Pre-scan argv so a clap parse error (which exits before `cli.json` is
    // populated) still routes through the JSON envelope path when requested.
    let json_mode_pre = env::args().any(|a| a == "--json");

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => return handle_parse_error(&e, json_mode_pre),
    };
    let json_mode = cli.json;

    let scout = match Scout::new().await {
        Ok(s) => s,
        Err(e) => return emit_error(&e, json_mode),
    };

    match scout.run(cli.command).await {
        Ok(output) => {
            let rendered = if json_mode {
                render_json_success(output)
            } else {
                output.markdown
            };
            let mut handle = stdout().lock();
            match write_output(&mut handle, &rendered) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(ErrorCode::IoError.exit_code())
                }
            }
        }
        Err(e) => emit_error(&e, json_mode),
    }
}

/// Serialize a successful `CommandOutput` as a one-line JSON envelope per ADR-0065.
/// Takes `CommandOutput` by value so `data` and `notes` move into the envelope
/// instead of being deep-cloned.
fn render_json_success(output: CommandOutput) -> String {
    let envelope = SuccessEnvelope {
        data: output.data,
        degraded: output.degraded,
        notes: output.notes,
    };
    serde_json::to_string(&envelope).expect("envelope is Serialize")
}

/// Serialize a `ScoutError` as a one-line JSON envelope per ADR-0065.
/// Uses `err.message()` (bare) so `next_step` is not duplicated in `message`.
fn render_json_error(err: &ScoutError) -> String {
    let envelope = ErrorEnvelope {
        error: ErrorPayload {
            code: err.error_kind(),
            message: err.message().to_owned(),
            next_step: err.next_step().map(str::to_owned),
            candidates: err.candidates().to_vec(),
            retryable: err.retryable(),
        },
    };
    serde_json::to_string(&envelope).expect("envelope is Serialize")
}

/// Handle a `clap::Error` from `Cli::try_parse()`. Help/version display
/// stay on stdout per clap convention; usage errors route through the JSON
/// envelope when `--json` was passed in argv.
fn handle_parse_error(err: &clap::Error, json_mode: bool) -> ExitCode {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = err.print();
            ExitCode::SUCCESS
        }
        _ => {
            if json_mode {
                let envelope = ErrorEnvelope {
                    error: ErrorPayload {
                        code: ErrorCode::UsageError,
                        message: err.to_string().trim().to_owned(),
                        next_step: None,
                        candidates: Vec::new(),
                        retryable: false,
                    },
                };
                let line = serde_json::to_string(&envelope).expect("envelope is Serialize");
                eprintln!("{line}");
            } else {
                let _ = err.print();
            }
            ExitCode::from(ErrorCode::UsageError.exit_code())
        }
    }
}

/// Print error to stderr (JSON envelope when `--json`, plain `error: <msg>` otherwise)
/// and return the appropriate `ExitCode`.
fn emit_error(err: &ScoutError, json_mode: bool) -> ExitCode {
    if json_mode {
        let line = render_json_error(err);
        eprintln!("{line}");
    } else {
        eprintln!("error: {err}");
    }
    ExitCode::from(err.exit_code())
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use clap::CommandFactory;

    use super::write_output;

    /// [T-W001] write_output appends newline when output lacks trailing newline
    #[test]
    fn write_output_appends_newline_when_missing() {
        let mut buf = Vec::new();
        write_output(&mut buf, "hello").unwrap();
        assert_eq!(&buf, b"hello\n");
    }

    /// [T-W002] write_output preserves single trailing newline
    #[test]
    fn write_output_preserves_existing_newline() {
        let mut buf = Vec::new();
        write_output(&mut buf, "hello\n").unwrap();
        assert_eq!(&buf, b"hello\n");
    }

    /// [T-W003] write_output propagates BrokenPipe error from writer
    #[test]
    fn write_output_propagates_broken_pipe() {
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

    /// [T-H000] root --help contains sysexits Exit codes and Environment sections
    #[test]
    fn root_help_contains_exit_codes_and_environment() {
        let help = super::Cli::command().render_long_help().to_string();
        assert!(
            help.contains("Exit codes"),
            "root help missing Exit codes section"
        );
        assert!(
            help.contains("sysexits.h"),
            "root help should reference sysexits.h"
        );
        assert!(
            help.contains("GEMINI_API_KEY"),
            "root help missing GEMINI_API_KEY"
        );
        assert!(
            help.contains("GITHUB_TOKEN"),
            "root help missing GITHUB_TOKEN"
        );
        for code in ["64", "65", "66", "74", "75"] {
            assert!(
                help.contains(code),
                "root help should document sysexits code {code}"
            );
        }
        assert!(
            help.contains("Usage error"),
            "root help missing EX_USAGE description"
        );
        assert!(
            help.contains("Temporary failure"),
            "root help missing EX_TEMPFAIL description"
        );
    }
}
