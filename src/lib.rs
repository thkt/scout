mod body_limit;
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
mod yaml;

const USER_AGENT: &str = concat!("scout/", env!("CARGO_PKG_VERSION"));

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

/// Allow CDP close (5s) and subprocess cleanup, with a bounded interrupt delay.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(7);

enum Outcome<T> {
    Completed(T),
    Interrupted(InterruptSignal),
}

fn write_output<W: Write>(w: &mut W, output: &str) -> io::Result<()> {
    if output.is_empty() {
        // Zero search results must not create a phantom line in URL pipelines.
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
struct Cli {
    /// Emit output as a JSON envelope (one line) on stdout
    /// instead of Markdown. Errors print a JSON envelope on stderr.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

/// Reuse an installed tracing subscriber when run is invoked again.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                // A parse fallback could silently widen INFO logging to all crates.
                "scout=info".parse().expect("static directive is valid"),
            ),
        )
        .try_init();
}

/// On interrupt, notify CDP cancellation and drain before dropping the command.
/// Injected futures let tests check this ordering without OS signal handlers.
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

    // Clap errors precede Cli construction, so detect JSON mode from argv.
    // args_os lets clap classify non-UTF-8 input instead of panicking first.
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
        Outcome::Completed(Ok(output)) => emit_success(output, json_mode, &mut stdout().lock()),
        Outcome::Completed(Err(e)) => emit_error(&e, json_mode),
        Outcome::Interrupted(sig) => {
            eprintln!("{}", interrupted_line(sig, json_mode));
            ExitCode::from(sig.exit_code())
        }
    }
}

/// Inject the writer to test CLI rendering and exit codes without replacing
/// process streams or exposing a production endpoint override.
fn emit_success<W: Write>(output: CommandOutput, json_mode: bool, writer: &mut W) -> ExitCode {
    let rendered = if json_mode {
        render_json_success(output)
    } else {
        output.into_markdown()
    };
    match write_output(writer, &rendered) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", write_failure_line(&e, json_mode));
            ExitCode::from(ErrorCode::IoError.exit_code())
        }
    }
}

/// Consume the output to move data and notes into the JSON envelope.
fn render_json_success(output: CommandOutput) -> String {
    to_json_line(&output.into_envelope())
}

/// Use the bare message so next_step is not duplicated in JSON.
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

/// Version-only discovery must still point agents to help. Log on stderr to
/// keep stdout parseable; init_tracing forces scout INFO even under RUST_LOG.
const AGENT_HELP_HINT: &str = "If you are a coding agent, run `scout --help` and `scout <command> --help` before answering questions about scout or troubleshooting its errors. The help output is authoritative for the installed version.";

/// Clap and write failures have no ScoutError; derive retryability from the
/// code rather than duplicating its mapping.
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

/// Write failures must obey the JSON stderr contract when requested.
fn write_failure_line(err: &io::Error, json_mode: bool) -> String {
    if json_mode {
        bare_error_line(ErrorCode::IoError, err.to_string())
    } else {
        format!("error: {err}")
    }
}

/// ADR-0017's caller retry on exit 130 is a strategy, not a claim of transient
/// failure; the JSON retryable flag remains false.
fn interrupted_line(sig: InterruptSignal, json_mode: bool) -> String {
    let message = format!("interrupted ({sig})");
    if json_mode {
        bare_error_line(interrupt_code(sig), message)
    } else {
        format!("error: {message}")
    }
}

/// JSON codes are platform-independent, unlike the cfg-gated signal variants.
fn interrupt_code(sig: InterruptSignal) -> ErrorCode {
    match sig {
        InterruptSignal::Sigint => ErrorCode::InterruptedSigint,
        #[cfg(unix)]
        InterruptSignal::Sigterm => ErrorCode::InterruptedSigterm,
    }
}

/// Keep clap help/version on stdout and honor JSON mode for usage errors.
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
    use std::process::ExitCode;

    use clap::CommandFactory;
    use tokio::sync::watch;

    use super::{
        CommandOutput, ErrorCode, InterruptSignal, Outcome, ScoutError, bare_error_line, drive,
        emit_success, init_tracing, interrupted_line, render_json_success, write_failure_line,
        write_output,
    };

    /// [T-DRV001] A ready SIGINT beats a pending command and maps to exit 130.
    /// Paused time avoids waiting for the drain timeout.
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

    /// [T-DRV002] Interrupting a pending command sets the cancellation flag.
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

    /// [T-DRV003] A ready command beats a pending signal without setting cancellation.
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

    /// [T-RJS001] Success JSON retains the fixture payload and degraded=false
    /// without literal newlines.
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

    /// [T-INIT001] A second tracing initialization must not panic.
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

    /// [T-W003] BrokenPipe returns 0 and StorageFull returns 74 through the CLI
    /// success writer in both output modes.
    #[test]
    fn emit_success_preserves_write_failure_exit_codes() {
        struct FailingWriter(io::ErrorKind);
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(self.0))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for json_mode in [false, true] {
            for (kind, expected) in [
                (io::ErrorKind::BrokenPipe, ExitCode::SUCCESS),
                (io::ErrorKind::StorageFull, ExitCode::from(74)),
            ] {
                let output = CommandOutput::ok(
                    String::from("hello"),
                    serde_json::json!({"markdown": "hello"}),
                );
                assert_eq!(
                    emit_success(output, json_mode, &mut FailingWriter(kind)),
                    expected,
                    "incorrect exit code for {kind:?} with json_mode={json_mode}"
                );
            }
        }
    }

    /// [T-W004] Write-failure rendering produces JSON IO_ERROR, retryable=false.
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

    /// [T-W007] Interruption JSON names the fixture signal and its error code,
    /// with retryable=false.
    #[test]
    fn interruption_is_an_envelope_under_json() {
        for (sig, code) in interrupt_signal_codes() {
            let line = interrupted_line(sig, true);
            let parsed: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("interruption must be valid JSON under --json: {e}"));
            assert_eq!(parsed["error"]["code"], serde_json::to_value(code).unwrap());
            assert_eq!(parsed["error"]["retryable"], false);
            assert!(
                parsed["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(&sig.to_string())),
                "the message names the signal, got: {line}"
            );
        }
    }

    /// [T-W008] without `--json` a signal interruption stays a plain line
    #[test]
    fn interruption_is_a_plain_line_without_json() {
        for (sig, _) in interrupt_signal_codes() {
            let line = interrupted_line(sig, false);
            assert_eq!(line, format!("error: interrupted ({sig})"));
        }
    }

    /// [T-W009] Separately defined signal and JSON exit codes must agree.
    #[test]
    fn the_interruption_code_matches_the_signal_exit_code() {
        for (sig, code) in interrupt_signal_codes() {
            assert_eq!(
                code.exit_code(),
                sig.exit_code(),
                "{code:?} and {sig} must agree on the exit code"
            );
        }
    }

    /// Every `InterruptSignal` paired with the `ErrorCode` its envelope carries.
    fn interrupt_signal_codes() -> Vec<(InterruptSignal, ErrorCode)> {
        vec![
            (InterruptSignal::Sigint, ErrorCode::InterruptedSigint),
            #[cfg(unix)]
            (InterruptSignal::Sigterm, ErrorCode::InterruptedSigterm),
        ]
    }

    /// [T-W006] Usage/IO failures are non-retryable; temporary failures/timeouts
    /// are retryable in the bare-error envelope.
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

    /// [T-H010] Root help names the tuning variables and Slack token.
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
