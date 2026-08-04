//! Slack message permalink parsing.

/// Parsed Slack message URL. [`parse_slack_url`] is the only path that
/// production code constructs one through, and it establishes the shape the
/// accessors below promise: non-empty workspace, `<secs>.<micros>` ts. Fields
/// are `pub(in crate::slack)` rather than private so test fixtures under
/// [`crate::slack`] can build one directly, which means a fixture's values
/// carry that shape by the author's care rather than by the parser.
#[derive(Debug, Clone)]
pub(crate) struct SlackUrl {
    pub(in crate::slack) workspace: String,
    pub(in crate::slack) channel: String,
    pub(in crate::slack) ts: String,
    pub(in crate::slack) thread_ts: Option<String>,
    pub(in crate::slack) raw_url: String,
}

impl SlackUrl {
    pub(crate) fn workspace(&self) -> &str {
        &self.workspace
    }

    pub(crate) fn channel(&self) -> &str {
        &self.channel
    }

    pub(crate) fn raw_url(&self) -> &str {
        &self.raw_url
    }
}

/// Parse a Slack message URL into its components.
///
/// Accepts `https://{workspace}.slack.com/archives/{channel}/p{ts_raw}[?thread_ts=…]`.
pub(crate) fn parse_slack_url(url: &str) -> Option<SlackUrl> {
    let parsed = url::Url::parse(url).ok()?;
    let workspace = parsed.host_str()?.strip_suffix(".slack.com")?;
    if workspace.is_empty() {
        return None;
    }

    let segments: Vec<&str> = parsed.path_segments()?.collect();
    if segments.len() != 3 || segments[0] != "archives" {
        return None;
    }

    let channel = segments[1].to_owned();
    // Slack timestamps: p{epoch_secs}{6-digit micros} → "{epoch_secs}.{micros}"
    const TS_MICROS_DIGITS: usize = 6;
    let ts_raw = segments[2].strip_prefix('p')?;
    if ts_raw.len() <= TS_MICROS_DIGITS {
        return None;
    }
    let (secs, micros) = ts_raw.split_at(ts_raw.len() - TS_MICROS_DIGITS);
    let ts = format!("{secs}.{micros}");

    let thread_ts = parsed
        .query_pairs()
        .find(|(k, _)| k == "thread_ts")
        .map(|(_, v)| v.into_owned());

    Some(SlackUrl {
        workspace: workspace.to_owned(),
        channel,
        ts,
        thread_ts,
        raw_url: url.to_owned(),
    })
}

#[cfg(test)]
mod url_tests;
