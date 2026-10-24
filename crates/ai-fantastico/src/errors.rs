//! FANTASTICO's error codes as the Craft AI layer's [`ErrorKind`]s, so an app knows whether to
//! ask for a key, a top-up, patience or a different request.

use craft_ai_core::{AiError, ErrorKind, SecretString};
use fantastico_sdk::Error;

/// The kind of failure an SDK error is.
pub(crate) fn kind_of(e: &Error) -> ErrorKind {
    match e.code.as_str() {
        "UNKNOWN_CREDENTIAL" | "INVALID_CONTO_KEY" | "INVALID_API_KEY" | "KEY_REVOKED" => {
            ErrorKind::Auth
        }
        "ALLOWANCE_EXHAUSTED" | "INSUFFICIENT_RESERVE" | "INSUFFICIENT_FUNDS" => ErrorKind::Quota,
        "NETWORK_ERROR" => ErrorKind::Network,
        // The provider refused this request's content or size: changing the request helps,
        // a new key or a wait does not.
        "INVALID_REQUEST"
        | "PROVIDER_CONTEXT_LENGTH_EXCEEDED"
        | "PROVIDER_CONTENT_POLICY_VIOLATION" => ErrorKind::InvalidRequest,
        "SURFACE_UNSUPPORTED" | "NOT_AVAILABLE" => ErrorKind::Unsupported,
        _ => match e.status {
            401 | 403 => ErrorKind::Auth,
            402 => ErrorKind::Quota,
            429 => ErrorKind::RateLimited,
            400 | 404 | 413 | 422 => ErrorKind::InvalidRequest,
            _ => ErrorKind::Provider,
        },
    }
}

/// What a person should read, with the account key scrubbed out.
pub(crate) fn to_ai(e: &Error, key: &SecretString) -> AiError {
    let kind = kind_of(e);
    let message = match kind {
        ErrorKind::Quota => {
            "The credit on the FANTASTICO account has run out: top it up to continue".to_owned()
        }
        ErrorKind::Network => format!("FANTASTICO cannot be reached: {}", e.message),
        _ => format!("{} ({})", e.message, e.code),
    };
    let message: String = message.chars().take(500).collect();
    AiError::redacted(kind, &message, &[key])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(code: &str, status: u16, message: &str) -> Error {
        Error {
            message: message.into(),
            code: code.into(),
            status,
        }
    }

    #[test]
    fn codes_and_statuses_map_to_kinds_an_app_can_act_on() {
        let key = SecretString::new("ck-fant-x");
        let cases = [
            (err("INVALID_CONTO_KEY", 401, "no"), ErrorKind::Auth),
            (err("UNKNOWN_CREDENTIAL", 400, "no"), ErrorKind::Auth),
            (err("ALLOWANCE_EXHAUSTED", 402, "no"), ErrorKind::Quota),
            (err("NETWORK_ERROR", 503, "down"), ErrorKind::Network),
            (
                err("PROVIDER_RATE_LIMITED", 429, "slow"),
                ErrorKind::RateLimited,
            ),
            (
                err("PROVIDER_CONTEXT_LENGTH_EXCEEDED", 400, "long"),
                ErrorKind::InvalidRequest,
            ),
            (
                err("PROVIDER_CONTENT_POLICY_VIOLATION", 403, "x"),
                ErrorKind::InvalidRequest,
            ),
            (err("SOMETHING_NEW", 403, "x"), ErrorKind::Auth),
            (err("SURFACE_UNSUPPORTED", 400, "x"), ErrorKind::Unsupported),
            (err("INTERNAL_ERROR", 500, "boom"), ErrorKind::Provider),
        ];
        for (e, kind) in cases {
            assert_eq!(to_ai(&e, &key).kind, kind, "{}", e.code);
        }
    }

    #[test]
    fn the_account_key_never_reaches_the_message() {
        let key = SecretString::new("ck-fant-secret-123");
        let e = to_ai(&err("X", 500, "bad key ck-fant-secret-123"), &key);
        assert!(!e.message.contains("secret-123"));
        assert!(
            to_ai(&err("ALLOWANCE_EXHAUSTED", 402, ""), &key)
                .message
                .contains("top it up")
        );
    }
}
