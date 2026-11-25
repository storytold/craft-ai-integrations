//! Where a task is polled, and how long "Task not found" is tolerated.
//!
//! BFL's global address routes a task to one of several clusters, and its docs say the
//! `polling_url` from the submit reply must be used to read it. That address receives the key, so
//! it is used only when it is https on `bfl.ai` or a subdomain (default port, no user-info). When
//! the API itself is on this computer (tests), only the same origin is accepted. Anything else is
//! ignored and the task is read from the configured address, `{base}/get_result?id=`.
//!
//! "Task not found" can be brief while a new task reaches its cluster, but BFL also uses it for
//! an expired or unknown task. The job fails once it has been not found on
//! [`NOT_FOUND_POLLS`] polls in a row *and* for at least the configured grace (60 seconds by
//! default), so fast polling cannot end it early and slow polling cannot keep it alive forever.
use craft_ai_core::endpoint::Endpoint;

/// Consecutive "Task not found" polls needed, with the grace, before the job fails.
pub(crate) const NOT_FOUND_POLLS: u32 = 10;
const MAX_LEN: usize = 2048;

/// The origin of `uri` (`scheme://host[:port]`), or `None` if it is not a clean http(s) address.
fn endpoint_of(uri: &str) -> Option<Endpoint> {
    if uri.len() > MAX_LEN || uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|s| uri.starts_with(s))?;
    let authority = uri[scheme.len()..].split(['/', '?', '#']).next()?;
    Endpoint::parse(&format!("{scheme}{authority}")).ok()
}

/// Whether the server's `polling_url` may be polled (and so be sent the key).
pub(crate) fn polling_url_allowed(uri: &str, base: &Endpoint) -> bool {
    let Some(endpoint) = endpoint_of(uri) else {
        return false;
    };
    if uri.contains('#') {
        return false;
    }
    if base.is_loopback() {
        return endpoint.fingerprint() == base.fingerprint();
    }
    endpoint.https
        && endpoint.port.is_none_or(|p| p == 443)
        && (endpoint.host == "bfl.ai" || endpoint.host.ends_with(".bfl.ai"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn official() -> Endpoint {
        Endpoint::parse("https://api.bfl.ai/v1").unwrap()
    }

    #[test]
    fn only_https_bfl_hosts_are_polled() {
        for good in [
            "https://api.us1.bfl.ai/v1/get_result?id=abc",
            "https://api.eu2.bfl.ai/v1/get_result?id=abc",
            "https://bfl.ai/v1/get_result?id=abc",
            "https://api.bfl.ai:443/v1/get_result?id=abc",
        ] {
            assert!(polling_url_allowed(good, &official()), "{good}");
        }
        for bad in [
            "http://api.us1.bfl.ai/v1/get_result?id=abc",
            "https://api.bfl.ai.evil.example/v1/get_result?id=abc",
            "https://evilbfl.ai/v1/get_result?id=abc",
            "https://user@api.bfl.ai/v1/get_result?id=abc",
            "https://api.bfl.ai:8443/v1/get_result?id=abc",
            "https://127.0.0.1/v1/get_result?id=abc",
            "https://api.bfl.ai/v1/get_result?id=a b",
            "https://api.bfl.ai/v1/get_result?id=a#frag",
            "ignored",
            "",
        ] {
            assert!(!polling_url_allowed(bad, &official()), "{bad}");
        }
    }

    #[test]
    fn a_loopback_api_only_polls_its_own_origin() {
        let base = Endpoint::parse("http://127.0.0.1:4000/v1").unwrap();
        assert!(polling_url_allowed(
            "http://127.0.0.1:4000/v1/elsewhere?id=1",
            &base
        ));
        assert!(!polling_url_allowed(
            "http://127.0.0.1:4001/v1/get_result?id=1",
            &base
        ));
        assert!(!polling_url_allowed(
            "https://api.us1.bfl.ai/v1/get_result?id=1",
            &base
        ));
    }
}
