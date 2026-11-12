//! Agreement to a workflow's unknown steps names those steps: it covers the same steps or
//! fewer, never a step it did not name. A record from before steps were named covers none.
use craft_ai_core::consent::DisclosureItem;
use craft_ai_core::*;

fn disclosure(steps: &[&str]) -> Disclosure {
    Disclosure {
        app: AppId::new("photocraft").unwrap(),
        connection: ConnectionId::new("comfyui-local").unwrap(),
        capability: CapabilityId::parse("image.generate").unwrap(),
        locality: Locality::Remote,
        destination: "http://127.0.0.1:8188".into(),
        items: vec![DisclosureItem {
            role: InputRole::Prompt,
            kind: PayloadKind::Text,
            bytes: Some(6),
        }],
        param_names: vec![],
        unknown_steps: steps.iter().map(|s| (*s).to_owned()).collect(),
    }
}

#[test]
fn agreement_covers_the_steps_it_named_and_no_others() {
    let mut ledger = ConsentLedger::default();
    ledger.grant(&disclosure(&["HttpPost", "WebhookSend"]));
    assert!(!ledger.needs_consent(&disclosure(&["HttpPost", "WebhookSend"]), 0));
    assert!(!ledger.needs_consent(&disclosure(&["HttpPost"]), 0));
    assert!(!ledger.needs_consent(&disclosure(&[]), 0));
    assert!(ledger.needs_consent(&disclosure(&["HttpPost", "Other"]), 0));
}

#[test]
fn agreement_for_no_steps_does_not_cover_a_workflow_with_some() {
    let mut ledger = ConsentLedger::default();
    ledger.grant(&disclosure(&[]));
    assert!(ledger.needs_consent(&disclosure(&["HttpPost"]), 0));
    ledger.grant(&disclosure(&["HttpPost"]));
    assert!(!ledger.needs_consent(&disclosure(&["HttpPost"]), 0));
}

#[test]
fn a_saved_record_without_steps_reads_as_none_and_steps_round_trip() {
    let mut ledger = ConsentLedger::default();
    ledger.grant(&disclosure(&[]));
    let old = serde_json::to_string(&ledger).unwrap();
    assert!(!old.contains("unknown_steps"), "{old}");
    let read: ConsentLedger = serde_json::from_str(&old).unwrap();
    assert!(read.needs_consent(&disclosure(&["HttpPost"]), 0));
    ledger.grant(&disclosure(&["HttpPost"]));
    let saved: ConsentLedger =
        serde_json::from_str(&serde_json::to_string(&ledger).unwrap()).unwrap();
    assert!(!saved.needs_consent(&disclosure(&["HttpPost"]), 0));
}
