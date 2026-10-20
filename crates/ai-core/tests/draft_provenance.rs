//! Connection-test results belong only to the address they were requested from.
use craft_ai_core::*;

fn draft(endpoint: &str) -> ConnectionDraft {
    let mut draft = ConnectionDraft::default();
    draft.label = "Test".into();
    draft.endpoint = endpoint.into();
    draft.uses_key = false;
    draft
}

fn models(id: &str) -> Vec<ModelInfo> {
    vec![ModelInfo::new(ModelId::new(id).unwrap(), id)]
}

#[test]
fn new_draft_discards_models_when_address_changes_after_probe() {
    let mut draft = draft("https://a.example/v1");
    assert!(draft.set_models("https://a.example/v1", models("a-only")));
    draft.endpoint = "https://b.example/v1".into();
    assert!(draft.prepare(&[]).unwrap().connection.models.is_empty());
}

#[test]
fn returning_to_original_address_does_not_keep_another_addresses_models() {
    let saved = draft("https://a.example/v1")
        .prepare(&[])
        .unwrap()
        .connection;
    let mut draft = ConnectionDraft::from_connection(&saved);
    draft.endpoint = "https://b.example/v1".into();
    assert!(draft.set_models("https://b.example/v1", models("b-only")));
    draft.endpoint = "https://a.example/v1".into();
    assert!(
        draft
            .prepare(&[saved])
            .unwrap()
            .connection
            .models
            .is_empty()
    );
}

#[test]
fn changing_model_contents_does_not_establish_address_provenance() {
    let saved = draft("https://a.example/v1")
        .prepare(&[])
        .unwrap()
        .connection;
    let mut draft = ConnectionDraft::from_connection(&saved);
    draft.models = models("refreshed-at-a");
    draft.endpoint = "https://b.example/v1".into();
    assert!(
        draft
            .prepare(&[saved])
            .unwrap()
            .connection
            .models
            .is_empty()
    );
}

#[test]
fn preset_models_belong_to_the_preset_address() {
    let preset = PRESETS
        .iter()
        .find(|preset| preset.key == "vertex-express")
        .unwrap();
    let mut draft = ConnectionDraft::from_preset(preset);
    draft.key = "test-key".into();
    assert!(!draft.prepare(&[]).unwrap().connection.models.is_empty());
    draft.endpoint = "https://other.example/v1".into();
    assert!(draft.prepare(&[]).unwrap().connection.models.is_empty());
}

#[test]
fn a_fresh_test_at_each_address_keeps_its_models() {
    let saved = draft("https://a.example/v1")
        .prepare(&[])
        .unwrap()
        .connection;
    let mut draft = ConnectionDraft::from_connection(&saved);
    for address in [
        "https://b.example/v1",
        "https://a.example/v1",
        "https://c.example/v1",
    ] {
        draft.endpoint = format!("  {address}  ");
        assert!(draft.set_models(address, models("same-model")));
        assert_eq!(
            draft
                .prepare(std::slice::from_ref(&saved))
                .unwrap()
                .connection
                .models,
            models("same-model")
        );
    }
}

#[test]
fn editing_and_returning_without_a_probe_preserves_original_models() {
    let mut saved = draft("https://a.example/v1")
        .prepare(&[])
        .unwrap()
        .connection;
    saved.models = models("a-only");
    let mut draft = ConnectionDraft::from_connection(&saved);
    draft.endpoint = "https://b.example/v1".into();
    assert!(
        draft
            .prepare(std::slice::from_ref(&saved))
            .unwrap()
            .connection
            .models
            .is_empty()
    );
    draft.endpoint = "https://a.example/v1".into();
    assert_eq!(
        draft
            .prepare(std::slice::from_ref(&saved))
            .unwrap()
            .connection
            .models,
        saved.models
    );
}

#[test]
fn custom_manual_models_still_work_for_a_new_connection_and_unchanged_address() {
    let mut draft = ConnectionDraft::custom();
    draft.endpoint = "https://custom.example/v1".into();
    draft.uses_key = false;
    draft.models = models("manual-model");
    let saved = draft.prepare(&[]).unwrap().connection;
    assert_eq!(saved.models, models("manual-model"));
    let mut edited = ConnectionDraft::from_connection(&saved);
    edited.models = models("other-manual-model");
    assert_eq!(
        edited.prepare(&[saved]).unwrap().connection.models,
        models("other-manual-model")
    );
}

#[test]
fn a_test_that_answers_after_the_address_moved_on_is_dropped() {
    let mut draft = draft("https://a.example/v1");
    // The test is started against a.example; the address is edited while it runs.
    draft.endpoint = "https://b.example/v1".into();
    assert!(
        !draft.set_models("https://a.example/v1", models("a-only")),
        "a late answer is not taken"
    );
    assert!(
        draft.models.is_empty(),
        "and it leaves the form's models alone"
    );
    assert!(draft.prepare(&[]).unwrap().connection.models.is_empty());
}

#[test]
fn a_test_that_answers_while_its_address_still_stands_is_taken() {
    let mut draft = draft("https://a.example/v1");
    assert!(
        draft.set_models("  https://a.example/v1  ", models("a-only")),
        "whitespace is not a different address"
    );
    assert_eq!(
        draft.prepare(&[]).unwrap().connection.models,
        models("a-only")
    );
}

#[test]
fn a_late_answer_does_not_overwrite_the_newer_address_own_models() {
    let mut draft = draft("https://a.example/v1");
    draft.endpoint = "https://b.example/v1".into();
    // The test at b.example answers first; the older one at a.example arrives afterwards.
    assert!(draft.set_models("https://b.example/v1", models("b-only")));
    assert!(!draft.set_models("https://a.example/v1", models("a-only")));
    assert_eq!(
        draft.prepare(&[]).unwrap().connection.models,
        models("b-only"),
        "the newer answer stands"
    );
}

#[test]
fn an_answer_is_judged_by_its_address_not_by_how_long_it_waited() {
    // Identity here is the address, not the test. After a.example → b.example → a.example, an
    // answer from the first test at a.example is still taken: the field holds a.example and those
    // models do belong to it. What must never happen is b.example's models being filed under
    // a.example, which is checked below.
    let mut draft = draft("https://a.example/v1");
    draft.endpoint = "https://b.example/v1".into();
    assert!(draft.set_models("https://b.example/v1", models("b-only")));
    draft.endpoint = "https://a.example/v1".into();
    assert!(
        draft.prepare(&[]).unwrap().connection.models.is_empty(),
        "b.example's models are not saved under a.example"
    );
    assert!(
        draft.set_models("https://a.example/v1", models("a-only")),
        "an answer for the address on the form is taken"
    );
    assert_eq!(
        draft.prepare(&[]).unwrap().connection.models,
        models("a-only")
    );
}

#[test]
fn a_refused_answer_leaves_models_already_on_the_form_untouched() {
    let mut draft = draft("https://a.example/v1");
    assert!(draft.set_models("https://a.example/v1", models("a-only")));
    draft.endpoint = "https://b.example/v1".into();
    assert!(!draft.set_models("https://a.example/v1", models("a-stale")));
    assert_eq!(
        draft.models,
        models("a-only"),
        "the refused set is not written over the form"
    );
}

#[test]
fn an_address_differing_only_in_case_or_a_trailing_slash_is_a_different_address() {
    // The comparison is on the field's text, not a parsed URL: Endpoint drops the path, so
    // /v1 and /v2 would look equal through it. Differing the other way is safe, the answer is
    // refused and the person tests again , so this pins which way it fails.
    for other in [
        "https://A.Example/v1",
        "https://a.example/v1/",
        "https://a.example/v2",
    ] {
        let mut draft = draft("https://a.example/v1");
        assert!(
            !draft.set_models(other, models("x")),
            "{other} is not the field's address"
        );
        assert!(draft.models.is_empty());
    }
}

#[test]
fn whitespace_in_a_saved_address_does_not_drop_its_models() {
    let mut saved = draft("https://a.example/v1")
        .prepare(&[])
        .unwrap()
        .connection;
    saved.endpoint = Some("  https://a.example/v1  ".into());
    saved.models = models("a-only");
    let form = ConnectionDraft::from_connection(&saved);
    assert_eq!(
        form.prepare(&[saved]).unwrap().connection.models,
        models("a-only")
    );
}
