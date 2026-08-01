use std::fmt;

/// Interrupt signal observed by `wait_for_signal`. Maps to a POSIX exit
/// convention so shell pipeline callers can distinguish user-initiated
/// interruption from internal errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterruptSignal {
    Sigint,
    #[cfg(unix)]
    Sigterm,
}

impl InterruptSignal {
    /// POSIX convention: 128 + signal number.
    /// SIGINT (2) → 130, SIGTERM (15) → 143.
    pub(crate) fn exit_code(self) -> u8 {
        match self {
            Self::Sigint => 130,
            #[cfg(unix)]
            Self::Sigterm => 143,
        }
    }
}

impl fmt::Display for InterruptSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sigint => "SIGINT",
            #[cfg(unix)]
            Self::Sigterm => "SIGTERM",
        })
    }
}

/// Wait until a process-terminating signal arrives.
///
/// On Unix: races SIGINT and SIGTERM. The first to fire wins.
/// On non-Unix: only SIGINT (via `ctrl_c()`).
///
/// If installing the SIGTERM handler fails, falls back to SIGINT-only
/// and logs a warning. SIGTERM in that case is handled by the runtime's
/// default disposition (immediate termination), which still triggers
/// `kill_on_drop` for the CDP child but skips the structured exit code.
pub(crate) async fn wait_for_signal() -> InterruptSignal {
    use tokio::signal::ctrl_c;

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGTERM handler; listening for SIGINT only");
                let _ = ctrl_c().await;
                return InterruptSignal::Sigint;
            }
        };
        tokio::select! {
            _ = ctrl_c() => InterruptSignal::Sigint,
            _ = term.recv() => InterruptSignal::Sigterm,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c().await;
        InterruptSignal::Sigint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-SIG001] SIGINT maps to 130 per the POSIX 128 + signal number convention
    #[test]
    fn sigint_exit_code_is_130() {
        assert_eq!(InterruptSignal::Sigint.exit_code(), 130);
    }

    /// [T-SIG002] SIGTERM maps to 143 per the POSIX 128 + signal number convention
    #[cfg(unix)]
    #[test]
    fn sigterm_exit_code_is_143() {
        assert_eq!(InterruptSignal::Sigterm.exit_code(), 143);
    }

    /// [T-SIG003] Sigint renders as the signal name "SIGINT"
    #[test]
    fn sigint_display_is_sigint() {
        assert_eq!(InterruptSignal::Sigint.to_string(), "SIGINT");
    }

    /// [T-SIG004] Sigterm renders as the signal name "SIGTERM"
    #[cfg(unix)]
    #[test]
    fn sigterm_display_is_sigterm() {
        assert_eq!(InterruptSignal::Sigterm.to_string(), "SIGTERM");
    }
}
