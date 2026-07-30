use axum::http::{header, HeaderMap};

use super::error::ApiError;
use crate::config::AdminToken;
use crate::state::AppState;

pub(crate) fn is_admin_host(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(config) = state.config.admin.as_ref() else {
        return false;
    };
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == config.host)
}

pub fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let config = state
        .config
        .admin
        .as_ref()
        .ok_or_else(ApiError::not_found)?;
    // Credential first: an unauthenticated caller learns nothing about which
    // Host or Origin the deployment expects. In insecure_no_auth mode the
    // bearer check is skipped — the operator has asserted that network-layer
    // controls substitute for it.
    if !config.insecure_no_auth {
        let token = config
            .token
            .as_ref()
            .expect("token is always Some when insecure_no_auth is false");
        authorize_bearer(token, headers)?;
    }
    if !is_admin_host(state, headers) {
        return Err(ApiError::forbidden());
    }
    if headers.get(header::ORIGIN).is_some_and(|origin| {
        origin
            .to_str()
            .map_or(true, |origin| !origin_matches_host(origin, &config.host))
    }) {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

/// Require exactly one `Authorization: Bearer <64 hex>` header matching the
/// configured operator token. Every rejection is the same 401 envelope.
fn authorize_bearer(token: &AdminToken, headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(ApiError::unauthorized());
    };
    let credential = value
        .to_str()
        .ok()
        .and_then(bearer_credential)
        .ok_or_else(ApiError::unauthorized)?;
    // Length and hex shape are public facts about the credential format, so
    // rejecting them early leaks nothing; only the value compare must be
    // constant-time.
    let mut presented = [0u8; 32];
    hex::decode_to_slice(credential, &mut presented)
        .map_err(|_| ApiError::unauthorized())
        .and_then(|()| {
            token
                .matches(&presented)
                .then_some(())
                .ok_or_else(ApiError::unauthorized)
        })
}

/// Extract the credential from an `Authorization` value. RFC 9110 makes the
/// auth scheme case-insensitive, so `bearer` is as valid as `Bearer`.
fn bearer_credential(value: &str) -> Option<&str> {
    let (scheme, credential) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("Bearer")
        .then(|| credential.trim_start_matches(' '))
        .filter(|credential| !credential.is_empty())
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        == Some(host)
}

#[cfg(test)]
mod tests {
    use super::{bearer_credential, origin_matches_host};

    #[test]
    fn browser_origin_must_match_admin_host() {
        assert!(origin_matches_host(
            "https://admin.example.com",
            "admin.example.com"
        ));
        assert!(origin_matches_host(
            "http://admin.localhost:3000",
            "admin.localhost:3000"
        ));
        assert!(!origin_matches_host(
            "https://attacker.example",
            "admin.example.com"
        ));
        assert!(!origin_matches_host("null", "admin.example.com"));
    }

    #[test]
    fn bearer_scheme_is_case_insensitive_and_credential_is_non_empty() {
        assert_eq!(bearer_credential("Bearer abc"), Some("abc"));
        assert_eq!(bearer_credential("bearer abc"), Some("abc"));
        assert_eq!(bearer_credential("BEARER  abc"), Some("abc"));
        assert_eq!(bearer_credential("Bearer "), None);
        assert_eq!(bearer_credential("Basic abc"), None);
        assert_eq!(bearer_credential("abc"), None);
        assert_eq!(bearer_credential(""), None);
    }
}
