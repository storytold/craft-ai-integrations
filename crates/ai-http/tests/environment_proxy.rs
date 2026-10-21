//! The proxy policy against a live environment: a proxy named by `ALL_PROXY` must reach
//! hosted bases and must never reach a loopback one.
//!
//! A test cannot set its own environment (`std::env::set_var` is unsafe, and this workspace
//! forbids unsafe code), so this test runs its own binary as a child with the variables set:
//! the child takes the marked branch and exits with the verdict.

const MARKER: &str = "CRAFT_AI_PROXY_TEST_CHILD";

#[test]
fn the_environments_proxy_reaches_hosted_bases_but_not_loopback_ones() {
    if std::env::var_os(MARKER).is_some() {
        // The child: ALL_PROXY names a proxy, and NO_PROXY cannot exempt anything.
        std::process::exit(
            if craft_ai_http::loopback_proxy().is_none()
                && craft_ai_http::hosted_proxy().is_some()
                && craft_ai_http::proxy_for("http://127.0.0.1:8188").is_none()
                && craft_ai_http::proxy_for("http://localhost:11434/v1").is_none()
                && craft_ai_http::proxy_for("https://api.example.com/v1").is_some()
            {
                0
            } else {
                1
            },
        );
    }
    let exe = std::env::current_exe().expect("this test binary exists");
    let status = std::process::Command::new(exe)
        .env(MARKER, "1")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env_remove("NO_PROXY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .status()
        .expect("this test binary can be run again");
    assert!(
        status.success(),
        "with ALL_PROXY set, loopback bases must refuse the proxy and hosted bases must take it"
    );
}
