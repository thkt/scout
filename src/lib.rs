mod brave;
mod charset;
mod classify;
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
use envelope::{CommandOutput, ErrorCode, ErrorEnvelope, ErrorPayload, to_json_line};
use signals::{InterruptSignal, wait_for_signal};
use tokio::sync::watch;
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
  SCOUT_GITHUB_TIMEOUT_SECS     repo-tree/repo-read/repo-overview wall-clock budget
                                (default 180, range 1-600)
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
    let _ = tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                // A literal in `target=level` form; `expect` so a future edit
                // that breaks the syntax fails loudly. The former fallback
                // dropped the target, widening the filter from scout to every
                // crate at INFO.
                "scout=info".parse().expect("static directive is valid"),
            ),
        )
        .try_init();
}

/// Race the in-flight command against signal arrival, returning the resulting
/// `Outcome`. On interrupt, notify the cancel handle so `fetch_with_cdp` can run
/// `browser.close()` (issue #121), then await the command for a bounded window
/// before returning the interrupt outcome.
///
/// Extracted from `run` with the signal source injected so the
/// signal-vs-command wiring (select → cancel notify → drain → `Interrupted`) is
/// unit-testable without spawning the real OS signal handlers (issue #228).
async fn drive<C, S>(
    cmd_fut: C,
    signal_fut: S,
    cancel: &watch::Sender<bool>,
) -> Outcome<Result<CommandOutput, ScoutError>>
where
    C: Future<Output = Result<CommandOutput, ScoutError>>,
    S: Future<Output = InterruptSignal>,
{
    tokio::pin!(cmd_fut);
    tokio::select! {
        res = &mut cmd_fut => Outcome::Completed(res),
        sig = signal_fut => {
            tracing::info!(signal = %sig, "interrupted, draining for graceful close");
            let _ = cancel.send(true);
            if timeout(SHUTDOWN_DRAIN_TIMEOUT, &mut cmd_fut).await.is_err() {
                tracing::warn!(
                    timeout_secs = SHUTDOWN_DRAIN_TIMEOUT.as_secs(),
                    "drain timed out; in-flight command was dropped before completion"
                );
            }
            Outcome::Interrupted(sig)
        }
    }
}

pub async fn run() -> ExitCode {
    init_tracing();

    // Pre-scan argv so a clap parse error (which exits before `cli.json` is
    // populated) still routes through the JSON envelope path when requested.
    // `args_os` rather than `args`: the latter panics on a non-UTF-8 argument,
    // which would abort with 101 before clap can classify it as a usage error.
    let json_mode_pre = env::args_os().any(|a| a == "--json");

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
    let outcome = drive(scout.run(cli.command), wait_for_signal(), &cancel).await;
    match outcome {
        Outcome::Completed(Ok(output)) => {
            let rendered = if json_mode {
                render_json_success(output)
            } else {
                output.into_markdown()
            };
            let mut handle = stdout().lock();
            match write_output(&mut handle, &rendered) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{}", write_failure_line(&e, json_mode));
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

/// Serialize a successful `CommandOutput` as a one-line JSON envelope per ADR-0010.
/// Takes `CommandOutput` by value so `data` and `notes` move into the envelope
/// instead of being deep-cloned.
fn render_json_success(output: CommandOutput) -> String {
    to_json_line(&output.into_envelope())
}

/// Serialize a `ScoutError` as a one-line JSON envelope per ADR-0010.
/// Uses `err.message()` (bare) so `next_step` is not duplicated in `message`.
fn render_json_error(err: &ScoutError) -> String {
    to_json_line(&ErrorEnvelope {
        error: ErrorPayload {
            code: err.error_kind(),
            message: err.message().to_owned(),
            next_step: err.next_step().map(str::to_owned),
            candidates: err.candidates().to_vec(),
            retryable: err.retryable(),
        },
    })
}

/// Points a coding agent at scout's own help output. An agent often runs
/// `--version` and nothing else before invoking a command, so the version
/// output is where the pointer has to live for it to be seen at all. Emitted
/// through the same tracing path as every other scout log, which puts it on
/// stderr and leaves the version line on stdout parseable. `init_tracing`
/// pins `scout=info` last, so no `RUST_LOG` value silences it; a caller that
/// wants it gone redirects stderr.
const AGENT_HELP_HINT: &str = "If you are a coding agent, run `scout --help` and `scout <command> --help` before answering questions about scout or troubleshooting its errors. The help output is authoritative for the installed version.";

/// One-line JSON envelope for a failure with no `ScoutError` behind it — a clap
/// parse error, or an stdout write that failed. `retryable` comes from the code
/// rather than being restated here, so the mapping lives only in [`ErrorCode`].
fn bare_error_line(code: ErrorCode, message: String) -> String {
    to_json_line(&ErrorEnvelope {
        error: ErrorPayload {
            code,
            message,
            next_step: None,
            candidates: Vec::new(),
            retryable: code.is_retryable(),
        },
    })
}

/// Render an stdout write failure for stderr. Under `--json` it has to be an
/// envelope: the flag tells callers every error on stderr is parseable, and a
/// bare line here would be the one place that promise breaks.
fn write_failure_line(err: &io::Error, json_mode: bool) -> String {
    if json_mode {
        bare_error_line(ErrorCode::IoError, err.to_string())
    } else {
        format!("error: {err}")
    }
}

/// Handle a `clap::Error` from `Cli::try_parse()`. Help/version display
/// stay on stdout per clap convention; usage errors route through the JSON
/// envelope when `--json` was passed in argv.
fn handle_parse_error(err: &clap::Error, json_mode: bool) -> ExitCode {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayVersion => {
            let _ = err.print();
            tracing::info!("{AGENT_HELP_HINT}");
            ExitCode::SUCCESS
        }
        ErrorKind::DisplayHelp => {
            let _ = err.print();
            ExitCode::SUCCESS
        }
        _ => {
            if json_mode {
                let line =
                    bare_error_line(ErrorCode::UsageError, err.to_string().trim().to_owned());
                eprintln!("{line}");
            } else {
                let _ = err.print();
            }
            ExitCode::from(ErrorCode::UsageError.exit_code())
        }
    }
}

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
    use std::future::{pending, ready};
    use std::io::{self, Write};

    use clap::CommandFactory;
    use tokio::sync::watch;

    use super::{
        CommandOutput, ErrorCode, InterruptSignal, Outcome, ScoutError, bare_error_line, drive,
        init_tracing, render_json_success, write_failure_line, write_output,
    };

    /// [T-DRV001] drive returns `Interrupted` carrying the firing signal, and that
    /// signal maps to its POSIX exit code, when the signal future resolves before
    /// the command. This is the issue #228 acceptance criterion: the
    /// signal → exit code wiring is exercised without the real OS signal handler.
    /// `start_paused` auto-advances the 7s drain timer so the pending command does
    /// not block for wall-clock time.
    #[tokio::test(start_paused = true)]
    async fn drive_interrupt_yields_signal_exit_code() {
        let (cancel, _rx) = watch::channel(false);
        let outcome = drive(
            pending::<Result<CommandOutput, ScoutError>>(),
            ready(InterruptSignal::Sigint),
            &cancel,
        )
        .await;
        let code = match outcome {
            Outcome::Interrupted(sig) => sig.exit_code(),
            Outcome::Completed(_) => panic!("expected interrupt, command never completes"),
        };
        assert_eq!(code, 130);
    }

    /// [T-DRV002] On interrupt, drive notifies the cancel handle so `fetch_with_cdp`
    /// can run `browser.close()` for graceful shutdown (issue #121).
    #[tokio::test(start_paused = true)]
    async fn drive_interrupt_notifies_cancel_handle() {
        let (cancel, rx) = watch::channel(false);
        let _ = drive(
            pending::<Result<CommandOutput, ScoutError>>(),
            ready(InterruptSignal::Sigint),
            &cancel,
        )
        .await;
        assert!(
            *rx.borrow(),
            "cancel handle must be notified so CDP can close gracefully"
        );
    }

    /// [T-DRV003] When the command completes before any signal, drive returns
    /// `Completed` and leaves the cancel handle untouched (no spurious shutdown).
    #[tokio::test]
    async fn drive_command_completion_wins_over_pending_signal() {
        let (cancel, rx) = watch::channel(false);
        let output = CommandOutput::ok(String::from("hi"), serde_json::json!({"markdown": "hi"}));
        let outcome = drive(
            ready(Ok::<_, ScoutError>(output)),
            pending::<InterruptSignal>(),
            &cancel,
        )
        .await;
        assert!(matches!(outcome, Outcome::Completed(Ok(_))));
        assert!(
            !*rx.borrow(),
            "cancel must not fire when the command completes normally"
        );
    }

    /// [T-RJS001] render_json_success serializes a `CommandOutput` as a one-line
    /// success envelope per ADR-0010: `data` payload preserved, `degraded:false`,
    /// no embedded newline. Pins the `--json` happy-path boundary so a regression
    /// in `into_envelope` / `to_json_line` wiring fails here.
    #[test]
    fn render_json_success_emits_one_line_success_envelope() {
        let output = CommandOutput::ok(
            String::from("hello"),
            serde_json::json!({"markdown": "hello"}),
        );
        let line = render_json_success(output);
        assert!(line.starts_with(r#"{"data":"#), "got: {line}");
        assert!(line.contains(r#""markdown":"hello""#), "got: {line}");
        assert!(line.contains(r#""degraded":false"#), "got: {line}");
        assert!(
            !line.contains('\n'),
            "envelope must be one line, got: {line}"
        );
    }

    /// [T-INIT001] init_tracing tolerates a second invocation (issue #103).
    /// `.init()` would panic on the duplicate; `.try_init()` returns Err which
    /// init_tracing silently ignores so callers (integration tests reusing
    /// `lib::run`) survive.
    #[test]
    fn init_tracing_is_idempotent() {
        init_tracing();
        init_tracing();
    }

    /// [T-W001]
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

    /// [T-W004] under `--json` a stdout write failure is reported as an envelope
    ///
    /// The flag promises every error on stderr is a JSON envelope, so a caller
    /// parsing stderr must not receive a bare line for this path.
    #[test]
    fn write_failure_is_an_envelope_under_json() {
        let err = io::Error::from(io::ErrorKind::StorageFull);
        let line = write_failure_line(&err, true);
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("write failure must be valid JSON under --json");
        assert_eq!(parsed["error"]["code"], "IO_ERROR");
        assert_eq!(parsed["error"]["retryable"], false);
    }

    /// [T-W005] without `--json` a stdout write failure stays a plain line
    #[test]
    fn write_failure_is_a_plain_line_without_json() {
        let err = io::Error::from(io::ErrorKind::StorageFull);
        let line = write_failure_line(&err, false);
        assert!(
            line.starts_with("error: "),
            "plain mode keeps the human-readable prefix, got: {line}"
        );
    }

    /// [T-W006] the bare-error envelope derives `retryable` from the code
    ///
    /// Restating it at the call site is how the two drift: `TempFailure` is
    /// retryable and `UsageError` is not, and only `ErrorCode` should say so.
    #[test]
    fn bare_error_line_derives_retryable_from_the_code() {
        for (code, expected) in [
            (ErrorCode::UsageError, false),
            (ErrorCode::IoError, false),
            (ErrorCode::TempFailure, true),
            (ErrorCode::Timeout, true),
        ] {
            let parsed: serde_json::Value =
                serde_json::from_str(&bare_error_line(code, "boom".to_owned()))
                    .expect("valid JSON");
            assert_eq!(
                parsed["error"]["retryable"], expected,
                "retryable for {code:?} should follow ErrorCode::is_retryable"
            );
        }
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

    /// [T-H010] root --help exposes SCOUT_* tuning env vars (issue #120).
    /// AI agents discover override knobs by reading --help; missing entries
    /// would force agents to read the source.
    #[test]
    fn root_help_lists_scout_tuning_env_vars() {
        let help = super::Cli::command().render_long_help().to_string();
        for var in [
            "SCOUT_FETCH_TIMEOUT_SECS",
            "SCOUT_RESEARCH_TIMEOUT_SECS",
            "SCOUT_SLACK_TIMEOUT_SECS",
            "SCOUT_GITHUB_TIMEOUT_SECS",
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
