//! Shared body-reading leaf used by the Brave, Slack, GitHub, and fetch
//! backends.
//!
//! Placement rule: a cap or helper lives here when 2 or more backends share
//! it. `read_body_capped` is used by all four backends, so it lives here.
//! `MAX_API_RESPONSE_BYTES` is shared by Brave and Slack, so it lives here
//! too. A cap used by exactly one backend stays with that backend instead:
//! `MAX_GITHUB_RESPONSE_BYTES` belongs in `github.rs`, and
//! `MAX_RESPONSE_BYTES` belongs in `fetch.rs`.

/// Upper bound on JSON response body bytes accepted from Brave and Slack
/// (issue #165 / CHX-008 / CHX-009). 1 MiB comfortably covers a
/// `web/search` payload at Brave's `count=20` default and a Slack thread
/// at `SLACK_REPLIES_LIMIT=200`; an oversized response cannot consume
/// unbounded memory while the JSON parser allocates. `fetch.rs` keeps a
/// separate `MAX_RESPONSE_BYTES = 10 MB` for HTML — the JSON cap is an
/// order of magnitude smaller because API payloads are structured data,
/// not human pages.
pub(crate) const MAX_API_RESPONSE_BYTES: usize = 1024 * 1024;

/// Drain `response` into a `Vec<u8>` while enforcing `cap` bytes. Content-Length
/// is pre-checked before any allocation; the chunk loop also rejects bodies that
/// exceed the cap when the header is absent or lies. Callers pass the cap that
/// matches their backend's legitimate payload size (`MAX_API_RESPONSE_BYTES`
/// above for Brave/Slack, `MAX_GITHUB_RESPONSE_BYTES` in `github.rs` for
/// GitHub, `MAX_RESPONSE_BYTES` in `fetch.rs` for HTML downloads).
///
/// `cap` applies to *decoded* bytes: with reqwest's compression features enabled,
/// `chunk()` yields already-decompressed data and `content_length()` returns
/// `None` for compressed responses (so the pre-check goes inert and the chunk
/// loop is the live guard). This bounds peak memory to `cap + one chunk` even
/// against a decompression bomb, at the cost of rejecting a legitimately large
/// page whose decompressed size exceeds the cap.
pub(crate) async fn read_body_capped<E>(
    response: reqwest::Response,
    cap: usize,
    too_large: impl Fn() -> E,
    network: impl Fn(reqwest::Error) -> E,
) -> Result<Vec<u8>, E> {
    let content_length = response.content_length();
    if let Some(len) = content_length
        && usize::try_from(len).unwrap_or(usize::MAX) > cap
    {
        return Err(too_large());
    }
    let capacity = content_length
        .map(|len| usize::try_from(len).unwrap_or(usize::MAX).min(cap))
        .unwrap_or(8192);
    let mut body = Vec::with_capacity(capacity);
    let mut stream = response;
    while let Some(chunk) = stream.chunk().await.map_err(&network)? {
        body.extend_from_slice(&chunk);
        if body.len() > cap {
            return Err(too_large());
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
