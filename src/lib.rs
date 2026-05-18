mod brave;
mod clock;
mod envelope;
mod fetch;
mod github;
mod markdown;
mod redacted;
mod retry;
mod rng;
mod search;
mod signals;
mod slack;
#[cfg(test)]
mod test_support;
mod token_source;
mod tools;

pub(crate) const USER_AGENT: &str = concat!("scout/", env!("CARGO_PKG_VERSION"));

use std::env;
use std::io::{self, ErrorKind, Write, stderr, stdout};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use envelope::{CommandOutput, ErrorCode, ErrorEnvelope, ErrorPayload, SuccessEnvelope};
use signals::{InterruptSignal, wait_for_signal};
use tokio::time::timeout;
use tools::{Command, Scout, ScoutError};

/// Maximum time the runtime waits for the in-flight command to wind down
/// after a SIGINT/SIGTERM. Long enough for CDP `browser.close()` (5s) plus
/// chromium's own subprocess cleanup margin; short enough not to feel like
/// a hang to the caller. Issue #121.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(7);

enum Outcome<T> {
    Completed(T),
    Interrupted(InterruptSignal),
}

fn write_output<W: Write>(w: &mut W, output: &str) -> io::Result<()> {
    if output.is_empty() {
        // Preserve true empty output (e.g., `scout search` with 0 results)
        // so line-oriented downstream callers don't see a phantom empty line.
        return Ok(());
    }
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
Exit codes (sysexits.h + GNU coreutils + POSIX signal convention):
  0    Success
  64   Usage error (clap parse, missing API key, conflicts_with violation)
  65   Data error (invalid input, malformed format, encoding error, 4xx body)
  66   Not found (repo/file not found, 404)
  70   Internal (scout-side invariant violation, unexpected response schema)
  74   IO error (external tool failure such as headless browser)
  75   Temporary failure (rate limit, 5xx, retryable — short backoff)
  104  Unknown (unclassifiable failure; rising rate signals classification gap)
  124  Timeout (request/transport timeout, retryable — longer backoff advised)
  130  Interrupted by SIGINT (128 + 2; e.g. Ctrl-C)
  143  Interrupted by SIGTERM (128 + 15; e.g. shell timeout, kill default)

Environment:
  BRAVE_SEARCH_API_KEY          Required for search and research commands
  GITHUB_TOKEN                  Optional for GitHub commands (higher rate limits)
  SLACK_TOKEN                   Optional. User OAuth token (xoxp-…) required for Slack URLs

Tuning (override built-in timeouts and retry budget):
  SCOUT_FETCH_TIMEOUT_SECS      fetch wall-clock budget per URL (default 95, range 1-600)
  SCOUT_RESEARCH_TIMEOUT_SECS   research wall-clock budget (default 45, range 1-600)
  SCOUT_SLACK_TIMEOUT_SECS      slack fetch wall-clock budget (default 60, range 1-600)
  SCOUT_MAX_RETRIES             retries on transient API failures, on top of the
                                initial attempt (default 2 → 3 total attempts,
                                range 0-10; set to 0 to disable retry)

Invalid tuning values fail with exit 64 (usage error) before any request is made."
)]
pub(crate) struct Cli {
    /// Emit output as a JSON envelope (one line) on stdout
    /// instead of Markdown. Errors print a JSON envelope on stderr.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

/// Install the tracing subscriber. `try_init` tolerates a second invocation
/// (e.g., integration tests that exercise `lib::run` more than once) — the
/// installed subscriber from the first call is reused.
fn init_tracing() {
    use tracing_subscriber::filter::Directive;
    let _ = tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                "scout=info"
                    .parse()
                    .unwrap_or_else(|_| Directive::from(tracing::Level::INFO)),
            ),
        )
        .try_init();
}

pub async fn run() -> ExitCode {
    init_tracing();

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
    let cancel = scout.cancel_handle();

    let cmd_fut = scout.run(cli.command);
    tokio::pin!(cmd_fut);

    // Race the in-flight command against signal arrival. On interrupt,
    // notify the cancel handle so fetch_with_cdp can run `browser.close()`
    // (issue #121) and then await the command for a bounded window before
    // returning the interrupt exit code.
    let outcome: Outcome<Result<CommandOutput, ScoutError>> = tokio::select! {
        res = &mut cmd_fut => Outcome::Completed(res),
        sig = wait_for_signal() => {
            tracing::info!(signal = %sig, "interrupted, draining for graceful close");
            let _ = cancel.send(true);
            let _ = timeout(SHUTDOWN_DRAIN_TIMEOUT, &mut cmd_fut).await;
            Outcome::Interrupted(sig)
        }
    };
    match outcome {
        Outcome::Completed(Ok(output)) => {
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
        Outcome::Completed(Err(e)) => emit_error(&e, json_mode),
        Outcome::Interrupted(sig) => {
            eprintln!("error: interrupted ({sig})");
            ExitCode::from(sig.exit_code())
        }
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
        degraded_reasons: output.degraded_reasons,
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

    use super::{init_tracing, write_output};

    /// [T-INIT001] init_tracing tolerates a second invocation (issue #103).
    /// `.init()` would panic on the duplicate; `.try_init()` returns Err which
    /// init_tracing silently ignores so callers (integration tests reusing
    /// `lib::run`) survive.
    #[test]
    fn init_tracing_is_idempotent() {
        init_tracing();
        init_tracing();
    }

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
            help.contains("BRAVE_SEARCH_API_KEY"),
            "root help missing BRAVE_SEARCH_API_KEY"
        );
        assert!(
            help.contains("GITHUB_TOKEN"),
            "root help missing GITHUB_TOKEN"
        );
        for code in [
            "64", "65", "66", "70", "74", "75", "104", "124", "130", "143",
        ] {
            assert!(
                help.contains(code),
                "root help should document sysexits/POSIX/GNU code {code}"
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
        assert!(
            help.contains("Internal"),
            "root help missing EX_SOFTWARE (70) description"
        );
        assert!(
            help.contains("Timeout"),
            "root help missing GNU timeout (124) description"
        );
        assert!(
            help.contains("Unknown"),
            "root help missing extension (104) description"
        );
        assert!(
            help.contains("SIGINT"),
            "root help missing SIGINT (130) description"
        );
        assert!(
            help.contains("SIGTERM"),
            "root help missing SIGTERM (143) description"
        );
    }

    /// [T-H001] root --help exposes SCOUT_* tuning env vars (issue #120).
    /// AI agents discover override knobs by reading --help; missing entries
    /// would force agents to read the source.
    #[test]
    fn root_help_lists_scout_tuning_env_vars() {
        let help = super::Cli::command().render_long_help().to_string();
        for var in [
            "SCOUT_FETCH_TIMEOUT_SECS",
            "SCOUT_RESEARCH_TIMEOUT_SECS",
            "SCOUT_SLACK_TIMEOUT_SECS",
            "SCOUT_MAX_RETRIES",
        ] {
            assert!(
                help.contains(var),
                "root help must list {var} so agents can discover the override"
            );
        }
        assert!(
            help.contains("SLACK_TOKEN"),
            "root help should list SLACK_TOKEN alongside other auth env vars"
        );
    }
}
