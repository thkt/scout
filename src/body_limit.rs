//! Body readers and limits shared by multiple backends. Backend-specific caps
//! stay with their backend; Brave and Slack share the JSON response cap here.

/// Upper bound on JSON response body bytes accepted from Brave and Slack.
///
/// 1 MiB comfortably covers a `web/search` payload at Brave's `count=20`
/// default and a Slack thread at `SLACK_REPLIES_LIMIT=200`; an oversized
/// response cannot consume unbounded memory while the JSON parser allocates.
/// `fetch.rs` keeps a separate `MAX_RESPONSE_BYTES = 10 MB` for HTML — the
/// JSON cap is an order of magnitude smaller because API payloads are
/// structured data, not human pages.
pub(crate) const MAX_API_RESPONSE_BYTES: usize = 1024 * 1024;

/// Limit the returned diagnostic prefix of a failed response.
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Read a diagnostic prefix; reaching `limit` is not an error.
/// `Response::text()` would buffer the entire failed response.
///
/// Returns at most `limit` bytes, reading through the chunk that reaches it
/// (or EOF). This helper does not cap chunk size. Vec capacity may exceed
/// `limit`; truncation reduces length without releasing capacity.
pub(crate) async fn read_body_snippet(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::new();
    while body.len() < limit {
        match response.chunk().await? {
            Some(chunk) => body.extend_from_slice(&chunk),
            None => break,
        }
    }
    body.truncate(limit);
    Ok(body)
}

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
/// loop is the live guard). This bounds the bytes read to `cap + one chunk`
/// even against a decompression bomb, at the cost of rejecting a legitimately
/// large page whose decompressed size exceeds the cap. The allocation follows
/// `cap` rather than the response: with Content-Length it is reserved once at
/// `min(len, cap)`, and without it the `Vec` doubles from 8 KiB until the loop
/// rejects.
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
    let capacity = content_length.map_or(8192, |len| {
        usize::try_from(len).unwrap_or(usize::MAX).min(cap)
    });
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
