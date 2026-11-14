//! A new agreement changes only the exact workflow-step set the person agreed to.

use craft_ai_core::consent::DisclosureItem;
use craft_ai_core::*;

/// One remote disclosure with the named steps and kinds, without any request contents.
fn disclosure(steps: &[&str], kinds: &[PayloadKind]) -> Disclosure {
    Disclosure {
        app: AppId::new("photocraft").unwrap(),
        connection: ConnectionId::new("comfyui-local").unwrap(),
        capability: CapabilityId::parse("image.generate").unwrap(),
        locality: Locality::Remote,
        destination: "http://127.0.0.1:8188".into(),
        items: kinds
            .iter()
            .map(|kind| DisclosureItem {
                role: InputRole::Source,
                kind: *kind,
                bytes: Some(6),
            })
            .collect(),
        param_names: vec![],
        unknown_steps: steps.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Image permission for one step must not become image permission for a wider workflow.
#[test]
fn a_narrow_image_grant_does_not_join_a_broader_text_grant() {
    let broad_text = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Text]);
    let narrow_image = disclosure(&["HttpPost"], &[PayloadKind::Image]);
    let broad_image = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Image]);
    let mut ledger = ConsentLedger::default();
    ledger.grant_until(&broad_text, 100);
    let before = ledger.records[0].clone();
    ledger.grant_until(&narrow_image, 100);
    assert!(ledger.needs_consent(&broad_image, 0));
    assert!(!ledger.needs_consent(&narrow_image, 0));
    assert!(!ledger.needs_consent(&broad_text, 0));
    assert_eq!(ledger.records[0], before);
    assert_eq!(ledger.records.len(), 2);
}

/// Renewing fewer steps leaves the broader agreement and its original expiry untouched.
#[test]
fn a_narrow_renewal_keeps_the_other_step_sets_original_expiry() {
    let broad = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Text]);
    let narrow = disclosure(&["HttpPost"], &[PayloadKind::Text]);
    let mut ledger = ConsentLedger::default();
    ledger.grant_until(&broad, 100);
    let before = ledger.records[0].clone();
    ledger.grant_until(&narrow, 200);
    assert_eq!(ledger.records[0], before);
    assert!(!ledger.needs_consent(&broad, 99));
    assert!(ledger.needs_consent(&broad, 100));
    assert!(!ledger.needs_consent(&narrow, 150));
    assert!(ledger.needs_consent(&narrow, 200));
}

/// A shorter grant for fewer steps cannot revoke the still-open broader agreement.
#[test]
fn a_narrow_timed_grant_keeps_an_open_broader_agreement() {
    let broad = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Text]);
    let narrow = disclosure(&["HttpPost"], &[PayloadKind::Text]);
    let mut ledger = ConsentLedger::default();
    ledger.grant(&broad);
    let before = ledger.records[0].clone();
    ledger.grant_until(&narrow, 100);
    assert_eq!(ledger.records[0], before);
    assert!(!ledger.needs_consent(&broad, u64::MAX));
    assert!(!ledger.needs_consent(&narrow, u64::MAX));
}

/// Step identity is a set: order and duplicate names do not prevent merging exact grants.
#[test]
fn exact_step_sets_merge_kinds_despite_order_and_duplicate_names() {
    let text = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Text]);
    let image = disclosure(
        &["WebhookSend", "HttpPost", "HttpPost"],
        &[PayloadKind::Image],
    );
    let mut ledger = ConsentLedger::default();
    ledger.grant_until(&text, 100);
    ledger.grant_until(&image, 100);
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(
        ledger.records[0].kinds,
        vec![PayloadKind::Text, PayloadKind::Image]
    );
    assert!(!ledger.needs_consent(&image, 99));
    assert!(!ledger.needs_consent(&text, 99));
    assert!(ledger.needs_consent(&image, 100));
}

/// Renewing one kind for the exact step set still leaves other kinds on their own expiry.
#[test]
fn an_exact_step_set_renewal_moves_only_the_named_kinds() {
    let both = disclosure(
        &["HttpPost", "WebhookSend"],
        &[PayloadKind::Text, PayloadKind::Image],
    );
    let text = disclosure(
        &["WebhookSend", "HttpPost", "HttpPost"],
        &[PayloadKind::Text],
    );
    let image = disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Image]);
    let mut ledger = ConsentLedger::default();
    ledger.grant_until(&both, 100);
    ledger.grant_until(&text, 200);
    assert_eq!(ledger.records.len(), 2);
    assert_eq!(ledger.records[0].kinds, vec![PayloadKind::Image]);
    assert_eq!(ledger.records[0].expires_at, Some(100));
    assert!(!ledger.needs_consent(&both, 99));
    assert!(ledger.needs_consent(&image, 100));
    assert!(!ledger.needs_consent(&text, 150));
    assert!(ledger.needs_consent(&text, 200));
}

/// A broader saved agreement continues to authorise a request using fewer of its steps.
#[test]
fn checking_permission_still_accepts_a_subset_of_the_agreed_steps() {
    let mut ledger = ConsentLedger::default();
    ledger.grant_until(
        &disclosure(&["HttpPost", "WebhookSend"], &[PayloadKind::Image]),
        100,
    );
    assert!(!ledger.needs_consent(
        &disclosure(&["HttpPost", "HttpPost"], &[PayloadKind::Image]),
        99,
    ));
    assert!(ledger.needs_consent(
        &disclosure(&["HttpPost", "Other"], &[PayloadKind::Image]),
        99,
    ));
}

/// Exact step names alone cannot merge grants for another app, connection, host or capability.
#[test]
fn matching_step_sets_do_not_merge_other_consent_scopes() {
    let text = disclosure(&["HttpPost"], &[PayloadKind::Text]);
    let image = disclosure(&["HttpPost"], &[PayloadKind::Image]);
    for changed in 0..4 {
        let mut other = image.clone();
        match changed {
            0 => other.app = AppId::new("wordcraft").unwrap(),
            1 => other.connection = ConnectionId::new("other-comfyui").unwrap(),
            2 => other.destination = "http://127.0.0.1:8189".into(),
            _ => other.capability = CapabilityId::parse("image.edit").unwrap(),
        }
        let mut ledger = ConsentLedger::default();
        ledger.grant_until(&text, 100);
        let before = ledger.records[0].clone();
        ledger.grant_until(&other, 100);
        assert!(ledger.needs_consent(&image, 0), "scope {changed}");
        assert!(!ledger.needs_consent(&other, 0), "scope {changed}");
        assert_eq!(ledger.records[0], before, "scope {changed}");
        assert_eq!(ledger.records.len(), 2, "scope {changed}");
    }
}
