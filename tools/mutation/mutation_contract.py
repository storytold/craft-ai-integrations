"""Regression guards for the contract fixes already merged in craft-libs.

Do not import the runner here: it executes immediately. Cases use its public tuple format and
name a focused runtime test so a compile error can never count as evidence.
"""
CORE = 'crates/ai-core/src/'
DRAFT = CORE + 'draft.rs'
ROLE = CORE + 'role.rs'
M = 'craft-ai-core'
CL = 'craft-ai-client'
CA = [
    ('sample-approval-ignores-changed-destination', 'crates/ai-client/src/tools_sample.rs',
     'if reviewed != Some(plan.disclosure()) {', 'if reviewed.is_none() {',
     CL, 'sample_consent', 'changed_destination_asks_again_before_saving_or_building'),
    ('sample-approval-compares-only-destination', 'crates/ai-client/src/tools_sample.rs',
     'if reviewed != Some(plan.disclosure()) {',
     'if reviewed.is_none_or(|shown| shown.destination != plan.disclosure().destination) {',
     CL, 'sample_consent', 'changed_payload_asks_again_before_saving_or_building'),
    ('late-model-answer-taken', DRAFT,
     'if address.trim() != self.endpoint.trim() {', 'if false {',
     M, 'draft_provenance', 'a_test_that_answers_after_the_address_moved_on'),
    ('probe-address-forgotten', DRAFT,
     'self.models_for = Some(self.endpoint.trim().to_owned());', 'self.models_for = None;',
     M, 'draft_provenance', 'new_draft_discards_models'),
    ('model-address-not-checked', DRAFT,
     '.is_none_or(|origin| origin == address)', '.is_none_or(|_| true)',
     M, 'draft_provenance', 'returning_to_original_address'),
    ('preset-address-forgotten', DRAFT,
     'models_for: (!models.is_empty()).then(|| preset.endpoint.trim().to_owned()),',
     'models_for: None,', M, 'draft_provenance', 'preset_models_belong'),
    ('loaded-model-address-forgotten', DRAFT,
     'models_for: c.endpoint.as_deref().map(|e| e.trim().to_owned()),', 'models_for: None,',
     M, 'draft_provenance', 'changing_model_contents'),
    ('loaded-address-not-trimmed', DRAFT,
     'models_for: c.endpoint.as_deref().map(|e| e.trim().to_owned()),',
     'models_for: c.endpoint.clone(),', M, 'draft_provenance', 'whitespace_in_a_saved_address'),
    ('picker-ignores-required-input', ROLE,
     '.all(|i| role.inputs.contains(i))', '.all(|_| true)',
     M, 'required_inputs', 'picker_does_not_offer'),
    ('binding-ignores-required-input', ROLE,
     'spec.requires.iter().find(|i| !role.inputs.contains(i))', 'spec.requires.iter().find(|_| false)',
     M, 'required_inputs', 'binding_refuses'),
    ('app-picker-ignores-required-input', ROLE,
     '.all(|i| role.inputs.contains(i))', '.all(|_| true)',
     CL, 'required_inputs', 'picker_omits'),
    ('app-status-and-plan-ignore-required-input', ROLE,
     'spec.requires.iter().find(|i| !role.inputs.contains(i))', 'spec.requires.iter().find(|_| false)',
     CL, 'required_inputs', 'status_and_plan'),
]
