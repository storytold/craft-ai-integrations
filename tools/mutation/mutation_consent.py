"""Deliberate-bug cases for group `ce`: the consent prompt's durations.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
S = 'crates/ai-egui/src/consent.rs'
EG = 'craft-ai-egui'

CE = [
    ('term-defaults-to-longest', S, 'term: craft_ai_core::ConsentTerm::ThisTime,\n            disclosure: None,', 'term: craft_ai_core::ConsentTerm::UntilRevoked,\n            disclosure: None,', EG, 'consent_terms', 'defaults_to_this_job'),
    ('changed-disclosure-keeps-term', S, '        state.term = craft_ai_core::ConsentTerm::ThisTime;\n        state.disclosure =', '        state.disclosure =', EG, 'consent_terms', 'changed_destination'),
    ('term-kept-after-an-answer', S, '*state = ConsentPromptState::default();', '{}', EG, 'consent_terms', 'finished_prompt'),
    ('deny-returns-a-permission', S, 'ConsentChoice::Deny => TimedConsentChoice::Deny,', 'ConsentChoice::Deny => TimedConsentChoice::Allow(state.term),', EG, 'consent_terms', 'refusing'),
    ('chosen-term-ignored', S, 'ConsentChoice::Allow => TimedConsentChoice::Allow(state.term),', 'ConsentChoice::Allow => TimedConsentChoice::Allow(craft_ai_core::ConsentTerm::ThisTime),', EG, 'consent_terms', 'choosing_a_duration'),
    ('original-prompt-offers-terms', S, 'draw_consent(ui, disclosure, app_label, None)', 'draw_consent(ui, disclosure, app_label, Some(&mut craft_ai_core::ConsentTerm::ThisTime))', EG, 'consent_terms', 'original_prompt'),
]

# Mutants no test can tell apart from the original, with the reason.
CE_EQUIVALENT = {
    'term-defaults-to-longest': 'the first frame of any prompt sees no disclosure in its state, which counts as a changed disclosure and resets the term to This time only before anything is drawn or returned; the default value is never observed',
}
