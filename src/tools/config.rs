use std::env;
use std::time::Duration;

use crate::retry::DEFAULT_MAX_RETRIES;

use super::errors::ScoutError;

const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 95;
const DEFAULT_RESEARCH_TIMEOUT_SECS: u64 = 45;
const DEFAULT_SLACK_TIMEOUT_SECS: u64 = 60;

const ENV_FETCH_TIMEOUT: &str = "SCOUT_FETCH_TIMEOUT_SECS";
const ENV_RESEARCH_TIMEOUT: &str = "SCOUT_RESEARCH_TIMEOUT_SECS";
const ENV_SLACK_TIMEOUT: &str = "SCOUT_SLACK_TIMEOUT_SECS";
const ENV_MAX_RETRIES: &str = "SCOUT_MAX_RETRIES";

const TIMEOUT_MIN_SECS: u64 = 1;
const TIMEOUT_MAX_SECS: u64 = 600;
/// Upper bound on the retry count. `10` was picked so a misconfigured
/// agent cannot starve scout by chaining requests; combined with the
/// default 1s+ backoff this caps the worst-case retry wall-clock around
/// 30s before the surrounding `*_TOOL_TIMEOUT` cuts in.
const RETRIES_CAP: u32 = 10;

/// Runtime tuning loaded from `SCOUT_*` env vars. Each field has a
/// hard-coded default and validates against a min/max range so a
/// misconfigured agent fails fast instead of running with an extreme value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeConfig {
    pub(crate) fetch_timeout: Duration,
    pub(crate) research_timeout: Duration,
    pub(crate) slack_timeout: Duration,
    pub(crate) max_retries: u32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            fetch_timeout: Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECS),
            research_timeout: Duration::from_secs(DEFAULT_RESEARCH_TIMEOUT_SECS),
            slack_timeout: Duration::from_secs(DEFAULT_SLACK_TIMEOUT_SECS),
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

impl RuntimeConfig {
    pub(crate) fn from_env() -> Result<Self, ScoutError> {
        Self::from_env_with(|k| env::var(k))
    }

    /// Wraps [`Self::from_env`] with a caller-supplied env reader so tests
    /// can exercise parse and range failures without
    /// `unsafe { std::env::set_var(...) }` (forbidden by `unsafe_code = "forbid"`).
    pub(crate) fn from_env_with<F>(get_var: F) -> Result<Self, ScoutError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        Ok(Self {
            fetch_timeout: parse_timeout(&get_var, ENV_FETCH_TIMEOUT, DEFAULT_FETCH_TIMEOUT_SECS)?,
            research_timeout: parse_timeout(
                &get_var,
                ENV_RESEARCH_TIMEOUT,
                DEFAULT_RESEARCH_TIMEOUT_SECS,
            )?,
            slack_timeout: parse_timeout(&get_var, ENV_SLACK_TIMEOUT, DEFAULT_SLACK_TIMEOUT_SECS)?,
            max_retries: parse_max_retries(&get_var)?,
        })
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

    /// [T-CFG001] env 未設定時はデフォルト値が使われる
    #[test]
    fn defaults_when_no_env_set() {
        let cfg = RuntimeConfig::from_env_with(empty_env).unwrap();
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(95));
        assert_eq!(cfg.research_timeout, Duration::from_secs(45));
        assert_eq!(cfg.slack_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG002] SCOUT_FETCH_TIMEOUT_SECS=120 は fetch_timeout に反映され、他はデフォルト
    #[test]
    fn fetch_timeout_override_reflects_only_fetch() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_FETCH_TIMEOUT_SECS", "120")).unwrap();
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(120));
        assert_eq!(cfg.research_timeout, Duration::from_secs(45));
        assert_eq!(cfg.slack_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG003] SCOUT_RESEARCH_TIMEOUT_SECS override が独立に効く
    #[test]
    fn research_timeout_override() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_RESEARCH_TIMEOUT_SECS", "30")).unwrap();
        assert_eq!(cfg.research_timeout, Duration::from_secs(30));
        assert_eq!(cfg.fetch_timeout, Duration::from_secs(95));
        assert_eq!(cfg.max_retries, 2);
    }

    /// [T-CFG004] SCOUT_SLACK_TIMEOUT_SECS override が独立に効く
    #[test]
    fn slack_timeout_override() {
        let cfg =
            RuntimeConfig::from_env_with(single_env("SCOUT_SLACK_TIMEOUT_SECS", "10")).unwrap();
        assert_eq!(cfg.slack_timeout, Duration::from_secs(10));
    }

    /// [T-CFG005] SCOUT_MAX_RETRIES=5 が max_retries に反映される
    #[test]
    fn max_retries_override() {
        let cfg = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "5")).unwrap();
        assert_eq!(cfg.max_retries, 5);
    }

    /// [T-CFG006] SCOUT_MAX_RETRIES=0 (retry 無効) を許容
    #[test]
    fn max_retries_zero_is_allowed() {
        let cfg = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "0")).unwrap();
        assert_eq!(cfg.max_retries, 0);
    }

    /// [T-CFG010] parse 失敗（英字混入）は UsageError(64) で fail-fast
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

    /// [T-CFG011] 空文字列は parse 失敗扱いで UsageError
    #[test]
    fn empty_value_fails_fast() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_MAX_RETRIES", "")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG012] 範囲外 timeout (0) は UsageError
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

    /// [T-CFG013] 範囲外 timeout (601) は UsageError
    #[test]
    fn timeout_above_max_fails() {
        let err = RuntimeConfig::from_env_with(single_env("SCOUT_RESEARCH_TIMEOUT_SECS", "601"))
            .unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG014] 範囲外 retries (11) は UsageError
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

    /// [T-CFG015] 負数は u64 parse 失敗で UsageError
    #[test]
    fn negative_value_fails() {
        let err =
            RuntimeConfig::from_env_with(single_env("SCOUT_SLACK_TIMEOUT_SECS", "-5")).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::UsageError);
    }

    /// [T-CFG020] Default impl は from_env_with(empty) と同値
    #[test]
    fn default_matches_empty_env() {
        let from_empty = RuntimeConfig::from_env_with(empty_env).unwrap();
        let from_default = RuntimeConfig::default();
        assert_eq!(from_empty.fetch_timeout, from_default.fetch_timeout);
        assert_eq!(from_empty.research_timeout, from_default.research_timeout);
        assert_eq!(from_empty.slack_timeout, from_default.slack_timeout);
        assert_eq!(from_empty.max_retries, from_default.max_retries);
    }
}
