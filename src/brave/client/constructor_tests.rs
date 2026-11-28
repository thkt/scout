use super::*;

/// [T-RC001] A key neither the env var nor the Keychain held surfaces as
/// `BraveError::ApiKeyNotSet`.
#[test]
fn from_secret_returns_api_key_not_set_without_a_key() {
    let result = BraveClient::from_secret(Client::new(), DEFAULT_MAX_RETRIES, None);
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected ApiKeyNotSet, got: {result:?}"
    );
}

/// [T-RC003] from_secret builds a client whose `api_key.expose()` returns the
/// resolved key and whose `base_url` is `API_BASE`.
#[test]
fn from_secret_constructs_client_with_api_base_and_exposed_key() {
    let result = BraveClient::from_secret(
        Client::new(),
        DEFAULT_MAX_RETRIES,
        Redacted::new("real-key"),
    );
    let client = result.expect("expected Ok(client) from valid key");
    assert_eq!(client.api_key.expose(), "real-key");
    assert_eq!(client.base_url, API_BASE);
}

/// [T-RC006] production constructor path must not enable the test-only HTTPS bypass.
/// `skip_https_check` is a `#[cfg(test)]` field; under `cargo test` it exists and
/// must be `false` when the client comes from `from_secret`.
#[test]
fn from_secret_does_not_set_skip_https_check() {
    let client = BraveClient::from_secret(Client::new(), DEFAULT_MAX_RETRIES, Redacted::new("k"))
        .expect("expected Ok(client) from valid key");
    assert!(
        !client.skip_https_check,
        "production constructor must not skip HTTPS check"
    );
}
