"""Deliberate-bug cases for group `mm`: models kept in memory by the built-in engine provider
(`ai-engine/src/memory.rs`, `provider.rs`). An unload never interrupts a run and is never lost,
an idle sweep leaves busy and recent models alone, one model's unload leaves the others, the size
shown follows what the model reports, and a load that will not fit is refused before anything
loads.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
M = 'crates/ai-engine/src/memory.rs'
P = 'crates/ai-engine/src/provider.rs'
E = 'craft-ai-engine'

BUSY = (
    'Err(TryLockError::WouldBlock) => {\n'
    '                if idle.is_some() {\n'
    '                    return Outcome::Untouched;\n'
    '                }\n'
    '                state.unload_requested = true;\n'
    '                return Outcome::AfterRun;\n'
    '            }'
)
IDLE_SKIP = (
    '                if idle.is_some() {\n'
    '                    return Outcome::Untouched;\n'
    '                }\n'
)
AT_LOAD = '                Some(bytes) => self.residency.loaded(bytes, false),\n'
AFTER_RUN = (
    '        if let Some(bytes) = reported(model.as_ref()) {\n'
    '            self.residency.loaded(bytes, false);\n'
    '        }\n'
)
CHECK = '            guarded(|| memory::check_fits(&self.model.label, needed, free))?;\n'
LOAD_START = '        if slot.is_none() {\n            let needed'

MM = [
    ('unload-request-ignored-on-leave', M, 'let taken = if discard || state.unload_requested {', 'let taken = if discard {', E, 'memory', 'in_use_is_unloaded_when_its_job_finishes'),
    ('busy-slot-taken-as-free', M, BUSY, 'Err(TryLockError::WouldBlock) => return Outcome::Untouched,', E, 'lib', 'unload_during_a_run_waits'),
    ('request-lost-before-enter', M, BUSY, BUSY.replace('                state.unload_requested = true;\n', ''), E, 'lib', 'between_taking_the_slot_and_entering'),
    ('idle-sweep-takes-recent-models', M, 'if state.held.is_none() || !rested {', 'if state.held.is_none() {', E, 'memory', 'idle_unload_frees_only_models_rested'),
    ('idle-sweep-marks-busy-models', M, IDLE_SKIP, '', E, 'memory', 'idle_unload_leaves_a_model_in_use_alone'),
    ('unload-leaves-model-listed', M, '        state.held = None;\n        state.unload_requested = false;\n        held.take()', '        state.unload_requested = false;\n        held.take()', E, 'memory', 'unloading_frees_the_model'),
    ('unload-one-takes-all', P, 'model.is_none_or(|m| &e.model.id == m)', 'true', E, 'memory', 'unloading_one_model_leaves_the_others'),
    ('memory-check-skipped', P, CHECK, '', E, 'memory', 'will_not_fit_is_refused'),
    ('reader-panic-not-guarded', P, CHECK, '            memory::check_fits(&self.model.label, needed, free)?;\n', E, 'memory', 'reader_that_panics'),
    ('memory-check-on-every-run', P, LOAD_START, '        memory::check_fits(&self.model.label, Some(1), free)?;\n' + LOAD_START, E, 'memory', 'runs_again_without_a_memory_check'),
    ('fit-compared-the-wrong-way', M, 'if needed <= free {', 'if needed >= free {', E, 'lib', 'fits_or_cannot_be_judged'),
    ('reported-size-ignored-at-load', P, AT_LOAD, '                Some(_) => self.residency.loaded(needed.unwrap_or_default(), true),\n', E, 'memory', 'reports_its_memory'),
    ('size-not-read-again-after-a-run', P, AFTER_RUN, '', E, 'memory', 'grows_between_runs'),
    ('bad-model-kept-after-a-crash', P, 'let discard = matches!(&result, Err(e) if e.kind == ErrorKind::Internal);', 'let discard = false;', E, 'provider', 'panicking_runner_fails_the_job_and_is_loaded_afresh'),
]
