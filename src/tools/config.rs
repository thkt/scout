use std::env;
use std::time::Duration;

use tracing::info;

use crate::retry::DEFAULT_MAX_RETRIES;

use super::errors::ScoutError;

const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 95;
const DEFAULT_RESEARCH_TIMEOUT_SECS: u64 = 45;
const DEFAULT_SLACK_TIMEOUT_SECS: u64 = 60;
/// Outer cap for a single GitHub command (`repo-tree` / `repo-read` /
/// `repo-overview`). Fail-fast bias by design (issue #185). 180s clears the
/// happy path of the most complex command — `repo-overview` runs a sequential
/// `get_repo`, four parallel calls, and a conditional README blob fetch, ~30s
/// `HTTP_TIMEOUT` each, so well under 180s when calls succeed — and clears every
/// cheap-retry path (5xx / rate-limit retries return in seconds) plus a typical
/// retried run where only some calls hit the 30s timeout. It sits *below* the
/// all-timeouts retry budget (~279s: three serial phases each retried 3× at the
/// 30s HTTP timeout), so a command whose upstream repeatedly hangs is cut rather
/// than waited out. Trade-off: a command where every phase exhausts its retries
/// on full 30s timeouts (~186s even without the blob fetch) is still cut.
/// Calibration is the operator's via `SCOUT_GITHUB_TIMEOUT_SECS`.
const DEFAULT_GITHUB_TIMEOUT_SECS: u64 = 180;

const ENV_FETCH_TIMEOUT: &str = "SCOUT_FETCH_TIMEOUT_SECS";
const ENV_RESEARCH_TIMEOUT: &str = "SCOUT_RESEARCH_TIMEOUT_SECS";
const ENV_SLACK_TIMEOUT: &str = "SCOUT_SLACK_TIMEOUT_SECS";
const ENV_GITHUB_TIMEOUT: &str = "SCOUT_GITHUB_TIMEOUT_SECS";
const ENV_MAX_RETRIES: &str = "SCOUT_MAX_RETRIES";

const TIMEOUT_MIN_SECS: u64 = 1;
const TIMEOUT_MAX_SECS: u64 = 600;
/// Upper bound on the retry count, so a misconfigured agent cannot starve scout
/// by chaining requests.
///
/// The cap does not bound wall-clock on its own: `jittered_backoff` doubles from
/// `INITIAL_BACKOFF_MS` and each sleep is capped at `MAX_RETRY_AFTER_SECS` (300),
/// so ten retries can sleep for roughly 800s in total. What cuts a run short is
/// the surrounding per-command budget — `fetch_timeout`, `research_timeout`,
/// `slack_timeout`, `github_timeout` — each overridable via its `SCOUT_*_SECS`
/// variable and itself capped at `TIMEOUT_MAX_SECS`.
const RETRIES_CAP: u32 = 10;

/// Runtime tuning loaded from `SCOUT_*` env vars. Each field has a
/// hard-coded default and validates against a min/max range so a
/// misconfigured agent fails fast instead of running with an extreme value.
#[derive(Debug, Clone, Copy)]
pub(super) struct RuntimeConfig {
    pub(super) fetch_timeout: Duration,
    pub(super) research_timeout: Duration,
    pub(super) slack_timeout: Duration,
    pub(super) github_timeout: Duration,
    pub(super) max_retries: u32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            fetch_timeout: Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECS),
            research_timeout: Duration::from_secs(DEFAULT_RESEARCH_TIMEOUT_SECS),
            slack_timeout: Duration::from_secs(DEFAULT_SLACK_TIMEOUT_SECS),
            github_timeout: Duration::from_secs(DEFAULT_GITHUB_TIMEOUT_SECS),
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

impl RuntimeConfig {
    pub(super) fn from_env() -> Result<Self, ScoutError> {
        Self::from_env_with(|k| env::var(k))
    }

    /// Wraps [`Self::from_env`] with a caller-supplied env reader so tests
    /// can exercise parse and range failures without
    /// `unsafe { std::env::set_var(...) }` (forbidden by `unsafe_code = "forbid"`).
    fn from_env_with<F>(get_var: F) -> Result<Self, ScoutError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        let config = Self {
            fetch_timeout: parse_timeout(&get_var, ENV_FETCH_TIMEOUT, DEFAULT_FETCH_TIMEOUT_SECS)?,
            research_timeout: parse_timeout(
                &get_var,
                ENV_RESEARCH_TIMEOUT,
                DEFAULT_RESEARCH_TIMEOUT_SECS,
            )?,
            slack_timeout: parse_timeout(&get_var, ENV_SLACK_TIMEOUT, DEFAULT_SLACK_TIMEOUT_SECS)?,
            github_timeout: parse_timeout(
                &get_var,
                ENV_GITHUB_TIMEOUT,
                DEFAULT_GITHUB_TIMEOUT_SECS,
            )?,
            max_retries: parse_max_retries(&get_var)?,
        };
        config.surface_overrides();
        Ok(config)
    }

    /// Emit one `info!` per `SCOUT_*` field whose value differs from the
    /// hard-coded default. Lets an operator inspect "which tuning is active"
    /// at the default log level without scanning the env for `SCOUT_*`.
    /// Silent when every field is on its default (no-op events are noise).
    fn surface_overrides(&self) {
        if self.fetch_timeout.as_secs() != DEFAULT_FETCH_TIMEOUT_SECS {
            info!(
                fetch_timeout_secs = self.fetch_timeout.as_secs(),
                "{ENV_FETCH_TIMEOUT} override applied"
            );
        }
        if self.research_timeout.as_secs() != DEFAULT_RESEARCH_TIMEOUT_SECS {
            info!(
                research_timeout_secs = self.research_timeout.as_secs(),
                "{ENV_RESEARCH_TIMEOUT} override applied"
            );
        }
        if self.slack_timeout.as_secs() != DEFAULT_SLACK_TIMEOUT_SECS {
            info!(
                slack_timeout_secs = self.slack_timeout.as_secs(),
                "{ENV_SLACK_TIMEOUT} override applied"
            );
        }
        if self.github_timeout.as_secs() != DEFAULT_GITHUB_TIMEOUT_SECS {
            info!(
                github_timeout_secs = self.github_timeout.as_secs(),
                "{ENV_GITHUB_TIMEOUT} override applied"
            );
        }
        if self.max_retries != DEFAULT_MAX_RETRIES {
            info!(
                max_retries = self.max_retries,
                "{ENV_MAX_RETRIES} override applied"
            );
        }
    }
}

/// Read an env var with the "unset means default, anything else must parse"
/// contract. `VarError::NotUnicode` is treated as a configured-but-invalid
/// value (UsageError), not as "absent" — the agent shouldn't accidentally
/// fall through to the default when the value was set but unreadable.
fn read_env_raw<F>(get_var: &F, key: &str) -> Result<Option<String>, ScoutError>
where
    F: Fn(&str) -> Result<String, env::VarError>,
{
    match get_var(key) {
        Ok(v) => Ok(Some(v)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(ScoutError::user_error(format!("{key} must be valid UTF-8")))
        }
    }
}

fn parse_timeout<F>(get_var: &F, key: &str, default: u64) -> Result<Duration, ScoutError>
where
    F: Fn(&str) -> Result<String, env::VarError>,
{
    let Some(raw) = read_env_raw(get_var, key)? else {
        return Ok(Duration::from_secs(default));
    };
    let secs: u64 = raw.trim().parse().map_err(|_| {
        ScoutError::user_error(format!(
            "{key} must be an integer between {TIMEOUT_MIN_SECS} and {TIMEOUT_MAX_SECS} seconds, got: {raw:?}"
        ))
    })?;
    if !(TIMEOUT_MIN_SECS..=TIMEOUT_MAX_SECS).contains(&secs) {
        return Err(ScoutError::user_error(format!(
            "{key} must be between {TIMEOUT_MIN_SECS} and {TIMEOUT_MAX_SECS} seconds, got: {secs}"
        )));
    }
    Ok(Duration::from_secs(secs))
}

fn parse_max_retries<F>(get_var: &F) -> Result<u32, ScoutError>
where
    F: Fn(&str) -> Result<String, env::VarError>,
{
    let Some(raw) = read_env_raw(get_var, ENV_MAX_RETRIES)? else {
        return Ok(DEFAULT_MAX_RETRIES);
    };
    let n: u32 = raw.trim().parse().map_err(|_| {
        ScoutError::user_error(format!(
            "{ENV_MAX_RETRIES} must be an integer between 0 and {RETRIES_CAP}, got: {raw:?}"
        ))
    })?;
    if n > RETRIES_CAP {
        return Err(ScoutError::user_error(format!(
            "{ENV_MAX_RETRIES} must be at most {RETRIES_CAP}, got: {n}"
        )));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use std::env::VarError;

    use crate::envelope::ErrorCode;

    use super::*;

    fn empty_env(_: &str) -> Result<String, VarError> {
        Err(VarError::NotPresent)
    }

    fn single_env(
        target: &'static str,
        value: &'static str,
    ) -> impl Fn(&str) -> Result<String, VarError> {
        move |k| {
            if k == target {
                Ok(value.to_owned())
            } else {
                Err(VarError::NotPresent)
            }
        }
    }

    /// [T-CFG001]
    #[test]
    fn defaults_when_no_env_set() {
        let cfg = RuntimeConfig::from_env_with(empty_env).unwrap();
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(95));
        assert_eq!(cfg.research_timeout, Duration::from_secs(45));
        assert_eq!(cfg.slack_timeout, Duration::from_secs(60));
        assert_eq!(cfg.github_timeout, Duration::from_secs(180));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG002]
    #[test]
    fn fetch_timeout_override_reflects_only_fetch() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_FETCH_TIMEOUT_SECS", "120")).unwrap();
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(120));
        assert_eq!(cfg.research_timeout, Duration::from_secs(45));
        assert_eq!(cfg.slack_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG003] The SCOUT_RESEARCH_TIMEOUT_SECS override changes research_timeout and
    /// leaves the other fields on their defaults
    #[test]
    fn research_timeout_override() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_RESEARCH_TIMEOUT_SECS", "30")).unwrap();
        assert_eq!(cfg.research_timeout, Duration::from_secs(30));
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(95));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG004] The SCOUT_SLACK_TIMEOUT_SECS override changes slack_timeout and leaves
    /// the other fields on their defaults
    #[test]
    fn slack_timeout_override() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_SLACK_TIMEOUT_SECS", "10")).unwrap();
        assert_eq!(cfg.slack_timeout, Duration::from_secs(10));
    }

    /// [T-CFG005] SCOUT_MAX_RETRIES=5 lands in max_retries
    #[test]
    fn max_retries_override() {
        let cfg = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "5")).unwrap();
        assert_eq!(cfg.max_retries, 5);
    }

    /// [T-CFG006] SCOUT_MAX_RETRIES=0 (retry disabled) is accepted
    #[test]
    fn max_retries_zero_is_allowed() {
        let cfg = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "0")).unwrap();
        assert_eq!(cfg.max_retries, 0);
    }

    /// [T-CFG010] A parse failure (letters mixed into the value) fails fast with UsageError(64)
    #[test]
    fn non_integer_value_fails_fast() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_FETCH_TIMEOUT_SECS", "abc"))
            .unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
        assert_eq!(err.exit_code(), 64);
        assert!(
            err.message().contains("SCOUT_FETCH_TIMEOUT_SECS"),
            "error should name the offending env var, got: {}",
            err.message()
        );
    }

    /// [T-CFG011] An empty string counts as a parse failure and yields UsageError
    #[test]
    fn empty_value_fails_fast() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG012] An out-of-range timeout (0) yields UsageError
    #[test]
    fn timeout_below_min_fails() {
        let err =
            RuntimeConfig::from_env_with(single_env("SCOUT_FETCH_TIMEOUT_SECS", "0")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
        assert!(
            err.message().contains("between"),
            "error should mention the valid range, got: {}",
            err.message()
        );
    }

    /// [T-CFG013]
    #[test]
    fn timeout_above_max_fails() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_RESEARCH_TIMEOUT_SECS", "601"))
            .unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG014] An out-of-range retry count (11) yields UsageError
    #[test]
    fn max_retries_above_cap_fails() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "11")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
        assert!(
            err.message().contains("at most"),
            "error should mention the cap, got: {}",
            err.message()
        );
    }

    /// [T-CFG015] A negative number fails the u64 parse and yields UsageError
    #[test]
    fn negative_value_fails() {
        let err =
            RuntimeConfig::from_env_with(single_env("SCOUT_SLACK_TIMEOUT_SECS", "-5")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG-LOG001] (issue #167 / OPS-005)
    /// Setup: env reader returns a non-default `SCOUT_FETCH_TIMEOUT_SECS`.
    /// Action: `RuntimeConfig::from_env_with(...)` runs under `traced_test`.
    /// Expected: an INFO event `SCOUT_FETCH_TIMEOUT_SECS override applied`
    /// fires with the structured `fetch_timeout_secs` field. The remaining
    /// `SCOUT_*` events stay silent because their fields are still on default.
    #[tracing_test::traced_test]
    #[test]
    fn fetch_timeout_override_surfaces_info_event() {
        let _ =
            RuntimeConfig::from_env_with(single_env("SCOUT_FETCH_TIMEOUT_SECS", "120")).unwrap();
        assert!(
            logs_contain("SCOUT_FETCH_TIMEOUT_SECS override applied"),
            "expected INFO event for the overridden field"
        );
        assert!(
            logs_contain("fetch_timeout_secs=120"),
            "expected structured field carrying the active value"
        );
        assert!(
            !logs_contain("SCOUT_RESEARCH_TIMEOUT_SECS override applied"),
            "unset fields must stay silent"
        );
    }

    /// [T-CFG-LOG002] All fields on default → no override events fire.
    /// Silent path; protects against a future regression that emits noise
    /// when nothing was overridden.
    #[tracing_test::traced_test]
    #[test]
    fn default_run_emits_no_override_events() {
        let _ = RuntimeConfig::from_env_with(empty_env).unwrap();
        assert!(
            !logs_contain("override applied"),
            "no-op runs must not emit override events"
        );
    }

    /// [T-CFG020]
    #[test]
    fn default_matches_empty_env() {
        let from_empty = RuntimeConfig::from_env_with(empty_env).unwrap();
        let from_default = RuntimeConfig::default();
        assert_eq!(from_empty.fetch_timeout, from_default.fetch_timeout);
        assert_eq!(from_empty.research_timeout, from_default.research_timeout);
        assert_eq!(from_empty.slack_timeout, from_default.slack_timeout);
        assert_eq!(from_empty.github_timeout, from_default.github_timeout);
        assert_eq!(from_empty.max_retries, from_default.max_retries);
    }

    /// [T-CFG021] The github_timeout default exceeds the inner HTTP and candidate-fetch timeouts
    ///
    /// When the outer GitHub-command timeout is at or below the inner
    /// per-request timeout, the outer one fires before a single request can
    /// finish. Pinning the hierarchy (outer > inner) as values catches a future
    /// change that shrinks an inner constant and breaks the inequality
    /// (issue #185).
    #[test]
    fn github_timeout_exceeds_inner_request_timeouts() {
        use crate::tools::builder::HTTP_TIMEOUT;
        use crate::tools::repo::CANDIDATE_FETCH_TIMEOUT;

        let github = RuntimeConfig::default().github_timeout;
        assert!(
            github > HTTP_TIMEOUT,
            "github_timeout ({github:?}) must exceed per-request HTTP_TIMEOUT"
        );
        assert!(
            github > CANDIDATE_FETCH_TIMEOUT,
            "github_timeout ({github:?}) must exceed CANDIDATE_FETCH_TIMEOUT"
        );
    }

    /// [T-CFG022]
    #[test]
    fn github_timeout_out_of_range_fails() {
        let err =
            RuntimeConfig::from_env_with(single_env("SCOUT_GITHUB_TIMEOUT_SECS", "0")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG-LOG003]
    #[tracing_test::traced_test]
    #[test]
    fn github_timeout_override_surfaces_info_event() {
        let _ =
            RuntimeConfig::from_env_with(single_env("SCOUT_GITHUB_TIMEOUT_SECS", "200")).unwrap();
        assert!(
            logs_contain("SCOUT_GITHUB_TIMEOUT_SECS override applied"),
            "expected INFO event for the overridden field"
        );
        assert!(
            logs_contain("github_timeout_secs=200"),
            "expected structured field carrying the active value"
        );
    }
}
