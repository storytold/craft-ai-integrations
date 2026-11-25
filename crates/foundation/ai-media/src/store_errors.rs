//! The store's error answers, and the one way a try can end that is not a failure: the kept
//! copy no longer fits the host's answer, so the store asks again at once for the whole file.

use craft_ai_core::{AiError, ErrorKind};

/// Why one request ended without a saved file.
pub(super) enum Stop {
    /// A real failure, judged by the retry rule.
    Fail(AiError),
    /// The kept copy was dropped because the host's answer does not continue it. Asking again
    /// for the whole file is part of the same try, not a new one.
    StartOver(&'static str),
}

impl From<AiError> for Stop {
    /// Any library error is a real failure.
    fn from(error: AiError) -> Self {
        Self::Fail(error)
    }
}

impl Stop {
    /// The error to report when the stop cannot be answered by asking again.
    pub(super) fn into_error(self) -> AiError {
        match self {
            Self::Fail(error) => error,
            Self::StartOver(why) => AiError::new(ErrorKind::Network, why),
        }
    }
}

/// Seconds from a `Retry-After` header. A date form is ignored, so the normal pause applies.
pub(super) fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// A transfer that stopped part way. Network, so the store tries again.
pub(super) fn transfer_failed() -> AiError {
    AiError::new(ErrorKind::Network, "media transfer failed")
}

/// The whole save, every try, redirect and pause included, ran past `transfer_timeout`.
pub(super) fn out_of_time() -> AiError {
    AiError::new(
        ErrorKind::Network,
        "the media download ran past its time limit",
    )
}

/// The disk would not take the file.
pub(super) fn disk(_: std::io::Error) -> AiError {
    AiError::new(ErrorKind::Internal, "could not write the media file")
}

/// The error for media past the configured size limit.
pub(super) fn too_large() -> AiError {
    AiError::new(
        ErrorKind::Provider,
        "the media is larger than the configured limit",
    )
}
