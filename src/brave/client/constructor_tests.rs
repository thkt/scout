use super::*;

/// [T-RC001] closure returning `Err(VarError::NotPresent)` must surface
/// as `BraveError::ApiKeyNotSet` from `from_env_with`. Exercises the injectable
/// env path that `from_env` delegates to. That path runs through the
/// backend-agnostic `Redacted::from_env_var`, which does not know `BraveError`.
#[test]
fn from_env_with_returns_api_key_not_set_when_closure_errs() {
    let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Err(env::VarError::NotPresent)
    });
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected ApiKeyNotSet, got: {result:?}"
    );
}

/// [T-RC002] Whitespace-only keys are rejected by the injected env reader.
#[test]
fn from_env_with_rejects_whitespace_only_key() {
    let result =
        BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("   ".to_owned()));
    assert!(
        matches!(result, Err(BraveError::ApiKeyNotSet)),
        "expected ApiKeyNotSet for whitespace-only key, got: {result:?}"
    );
}

/// [T-RC003] from_env_with builds a client whose `api_key.expose()` returns the
/// closure's key and whose `base_url` is `API_BASE`.
#[test]
fn from_env_with_constructs_client_with_api_base_and_exposed_key() {
    let result = BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| {
        Ok("real-key".to_owned())
    });
    let client = result.expect("expected Ok(client) from valid key");
    assert_eq!(client.api_key.expose(), "real-key");
    assert_eq!(client.base_url, API_BASE);
}

/// [T-RC006] production constructor path must not enable the test-only HTTPS bypass.
/// `skip_https_check` is a `#[cfg(test)]` field; under `cargo test` it exists and
/// must be `false` when the client comes from `from_env_with`.
#[test]
fn from_env_with_does_not_set_skip_https_check() {
    let client =
        BraveClient::from_env_with(Client::new(), DEFAULT_MAX_RETRIES, |_| Ok("k".to_owned()))
            .expect("expected Ok(client) from valid key");
    assert!(
        !client.skip_https_check,
        "production constructor must not skip HTTPS check"
    );
}
