//! A custom connection's test lists no models: the models a person declared are what the
//! connection offers. Its answer confirms the address and must not replace them with nothing.

use super::{ConnectionManager, ConnectionPanel, Duration, TestCapture, TestLine};
use craft_ai_client::{ProbeReport, TestReport};
use craft_ai_core::{ModelId, ModelInfo};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use std::sync::Arc;

/// A custom connection's answer for the form's own address keeps the declared models and passes.
#[test]
fn a_custom_connection_test_keeps_the_declared_models() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let manager = ConnectionManager::new(shared, Arc::new(MemoryStore::new()));
    let mut panel = ConnectionPanel::new(manager);
    panel.form.draft.endpoint = "http://127.0.0.1:9/v1".to_owned();
    panel.form.draft.models = vec![ModelInfo::new(
        ModelId::new("declared").unwrap(),
        "Declared",
    )];
    panel.testing = Some(TestCapture {
        endpoint: "http://127.0.0.1:9/v1".to_owned(),
        generation: panel.generation,
    });

    panel.finish_test(Ok(TestReport {
        models: Vec::new(),
        elapsed: Duration::ZERO,
        probe: Some(ProbeReport {
            confirmed: Vec::new(),
            unconfirmed: Vec::new(),
            models: Vec::new(),
            skipped: 0,
            elapsed: Duration::ZERO,
        }),
        unavailable_choices: Vec::new(),
    }));

    assert_eq!(panel.form.draft.models.len(), 1, "the declared model stays");
    assert!(matches!(panel.form.test, TestLine::Passed { .. }));
}

/// A passed test answer from a custom connection, which lists no models of its own.
fn probe_answer() -> Result<TestReport, craft_ai_core::AiError> {
    Ok(TestReport {
        models: Vec::new(),
        elapsed: Duration::ZERO,
        probe: Some(ProbeReport {
            confirmed: Vec::new(),
            unconfirmed: Vec::new(),
            models: Vec::new(),
            skipped: 0,
            elapsed: Duration::ZERO,
        }),
        unavailable_choices: Vec::new(),
    })
}

/// Moving a custom connection to a new address and testing it there keeps the declared models on
/// save. Before, the models still belonged to the old address, so the save stored none.
#[test]
fn a_passed_custom_test_at_a_new_address_keeps_the_declared_models_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let manager = ConnectionManager::new(shared, Arc::new(MemoryStore::new()));
    let mut panel = ConnectionPanel::new(manager);
    panel.form.draft.label = "Custom".to_owned();
    panel.form.draft.uses_key = false;
    panel.form.draft.endpoint = "http://127.0.0.1:9/v1".to_owned();
    let declared = vec![ModelInfo::new(
        ModelId::new("declared").unwrap(),
        "Declared",
    )];
    assert!(
        panel
            .form
            .draft
            .set_models("http://127.0.0.1:9/v1", declared)
    );

    panel.form.draft.endpoint = "http://127.0.0.1:10/v1".to_owned();
    panel.testing = Some(TestCapture {
        endpoint: "http://127.0.0.1:10/v1".to_owned(),
        generation: panel.generation,
    });
    panel.finish_test(probe_answer());

    assert!(matches!(panel.form.test, TestLine::Passed { .. }));
    let prepared = panel.form.draft.prepare(&[]).unwrap();
    assert_eq!(
        prepared.connection.models.len(),
        1,
        "the declared model is saved for the tested address"
    );
}

/// An answer for the address the form had before it changed confirms nothing: the declared models
/// stay with the address they were set for, and the new address is not marked as tested.
#[test]
fn a_custom_answer_for_an_old_address_confirms_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let manager = ConnectionManager::new(shared, Arc::new(MemoryStore::new()));
    let mut panel = ConnectionPanel::new(manager);
    panel.form.draft.label = "Custom".to_owned();
    panel.form.draft.uses_key = false;
    panel.form.draft.endpoint = "http://127.0.0.1:9/v1".to_owned();
    let declared = vec![ModelInfo::new(
        ModelId::new("declared").unwrap(),
        "Declared",
    )];
    assert!(
        panel
            .form
            .draft
            .set_models("http://127.0.0.1:9/v1", declared)
    );
    panel.testing = Some(TestCapture {
        endpoint: "http://127.0.0.1:9/v1".to_owned(),
        generation: panel.generation,
    });
    panel.form.draft.endpoint = "http://127.0.0.1:10/v1".to_owned();
    panel.finish_test(probe_answer());

    assert!(!matches!(panel.form.test, TestLine::Passed { .. }));
    let prepared = panel.form.draft.prepare(&[]).unwrap();
    assert!(
        prepared.connection.models.is_empty(),
        "the new address was never tested"
    );
}
