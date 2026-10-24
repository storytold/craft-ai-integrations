"""Deliberate-bug cases for group `eg`: the connection panel's lifecycle guards.

Loaded by mutation_check.py. Each case breaks one rule and names an existing ai-egui test that
must fail. The runner restores the original file in its `finally` block and writes the diagnostic
receipt to `tools/mutation/verification/eg-mutations.json` with per-case logs beside it.

Format: (name, file, old, new, package, test target, test filter).
"""
PANEL = 'crates/ai-egui/src/panel.rs'
EG = 'craft-ai-egui'

EG_CASES = [
    # A forced second UI pass arrives after `poll` drained the queued Save. Without the frame
    # dedup, the same press queues another store write and reports Saved twice.
    ('save-frame-dedup-removed', PANEL, 'if self.save_frame != Some(frame)\n                    && !self', 'if !self', EG, 'lib', 'panel::tests::a_save_consumed_by_poll_is_still_deduplicated_within_the_same_frame'),
    # The finish guard has two independent checks. Removing the generation half must still be
    # caught by the existing stale-answer unit test (same endpoint, older generation).
    ('answer-generation-ignored', PANEL, 'if capture.generation == self.generation\n            && capture.endpoint == self.form.draft.endpoint.trim()', 'if capture.endpoint == self.form.draft.endpoint.trim()', EG, 'lib', 'panel::tests::a_stale_answer_is_dropped_by_the_panel_guard'),
]
