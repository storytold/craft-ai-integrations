//! A custom connection's test lists no models: the models a person declared are what the
//! connection offers. Its answer confirms the address and must not replace them with nothing.

use super::*;
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
