//! A key sent over plain http to an address on this computer reaches whichever program holds
//! that port, which on a shared computer can be another account's. Such a request is disclosed
//! as leaving this computer, so consent is asked. Pure checks; nothing is sent.
use craft_ai_core::*;

fn connection(endpoint: &str, keyed: bool, locality: &str) -> Connection {
    let auth = if keyed {
        serde_json::json!({"kind": "api_key", "secret": "c-key"})
    } else {
        serde_json::json!({"kind": "none"})
    };
    serde_json::from_value(serde_json::json!({
        "id": "c", "label": "C", "provider": "openai-compatible", "endpoint": endpoint,
        "locality": locality, "auth": auth, "billing": "free", "models": []
    }))
    .unwrap()
}

fn disclosure(c: &Connection) -> Disclosure {
    let request = Request::new(CapabilityId::parse("text.complete").unwrap())
        .with_input(InputRole::Prompt, Payload::text("hi"));
    Disclosure::for_request(&AppId::new("wordcraft").unwrap(), c, &request)
}

#[test]
fn a_key_over_plain_http_to_this_computer_needs_agreement() {
    for endpoint in [
        "http://127.0.0.1:11434/v1",
        "http://localhost:1234/v1",
        "http://[::1]:8080/v1",
    ] {
        let d = disclosure(&connection(endpoint, true, "local"));
        assert_eq!(d.locality, Locality::Remote, "{endpoint}");
        assert!(d.key_in_clear, "{endpoint}");
        assert!(ConsentLedger::default().needs_consent(&d, 0), "{endpoint}");
        assert_eq!(d.destination, fingerprint(endpoint));
    }
}

#[test]
fn no_key_or_https_keeps_the_connection_local() {
    for (endpoint, keyed) in [
        ("http://127.0.0.1:11434/v1", false),
        ("https://127.0.0.1:8443/v1", true),
        ("https://localhost/v1", true),
    ] {
        let d = disclosure(&connection(endpoint, keyed, "local"));
        assert_eq!(d.locality, Locality::Local, "{endpoint}");
        assert!(!d.key_in_clear, "{endpoint}");
        assert!(!ConsentLedger::default().needs_consent(&d, 0), "{endpoint}");
    }
}

#[test]
fn a_remote_connection_stays_remote_and_one_agreement_covers_the_local_key() {
    let d = disclosure(&connection("https://api.example.com/v1", true, "remote"));
    assert_eq!(d.locality, Locality::Remote);
    assert!(!d.key_in_clear, "a key over https is not sent in the clear");
    let d = disclosure(&connection("http://127.0.0.1:11434/v1", true, "local"));
    let mut ledger = ConsentLedger::default();
    ledger.grant(&d);
    assert!(!ledger.needs_consent(&d, 0));
    // A different port is a different program, so it asks again.
    let other = disclosure(&connection("http://127.0.0.1:11435/v1", true, "local"));
    assert!(ledger.needs_consent(&other, 0));
}

/// The destination the disclosure names: scheme, host and port, as consent records it.
fn fingerprint(endpoint: &str) -> String {
    craft_ai_core::endpoint::Endpoint::parse(endpoint)
        .unwrap()
        .fingerprint()
}
