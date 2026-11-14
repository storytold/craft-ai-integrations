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
CHECK = '            let admitted = guarded(|| admission.admit(&self.model.label, needed, free))?;\n'
LOAD_START = '        if slot.is_none() {\n            let needed'

MM = [
    ('unload-request-ignored-on-leave', M, 'let taken = if discard || state.unload_requested {', 'let taken = if discard {', E, 'memory', 'in_use_is_unloaded_when_its_job_finishes'),
    ('busy-slot-taken-as-free', M, BUSY, 'Err(TryLockError::WouldBlock) => return Outcome::Untouched,', E, 'lib', 'unload_during_a_run_waits'),
    ('request-lost-before-enter', M, BUSY, BUSY.replace('                state.unload_requested = true;\n', ''), E, 'lib', 'between_taking_the_slot_and_entering'),
    ('idle-sweep-takes-recent-models', M, 'if state.held.is_none() || !rested {', 'if state.held.is_none() {', E, 'memory', 'idle_unload_frees_only_models_rested'),
    ('idle-sweep-marks-busy-models', M, IDLE_SKIP, '', E, 'memory', 'idle_unload_leaves_a_model_in_use_alone'),
    ('unload-leaves-model-listed', M, '        state.held = None;\n        state.unload_requested = false;\n        held.take()', '        state.unload_requested = false;\n        held.take()', E, 'memory', 'unloading_frees_the_model'),
    ('unload-one-takes-all', P, 'model.is_none_or(|m| &e.model.id == m)', 'true', E, 'memory', 'unloading_one_model_leaves_the_others'),
    ('memory-check-skipped', P, CHECK, '            let admitted = ();\n', E, 'memory', 'will_not_fit_is_refused'),
    ('reader-panic-not-guarded', P, CHECK, '            let admitted = admission.admit(&self.model.label, needed, free)?;\n', E, 'memory', 'reader_that_panics'),
    ('memory-check-on-every-run', P, LOAD_START, '        drop(admission.admit(&self.model.label, Some(1), free)?);\n' + LOAD_START, E, 'memory', 'runs_again_without_a_memory_check'),
    ('admitted-load-sets-nothing-aside', M, '        *set_aside = set_aside.saturating_add(bytes);\n', '', E, 'memory', 'another_load_has_been_promised'),
    ('set-aside-ignored-when-judging', M, '.map(|bytes| bytes.saturating_sub(*set_aside));', ';', E, 'memory', 'another_load_has_been_promised'),
    ('share-never-given-back', M, '        *set_aside = set_aside.saturating_sub(self.bytes);\n', '', E, 'memory', 'another_load_has_been_promised'),
    ('admission-not-shared', P, '        self.admission = admission;\n', '', E, 'memory', 'sharing_an_admission'),
    ('fit-compared-the-wrong-way', M, 'if needed <= free {', 'if needed >= free {', E, 'lib', 'fits_or_cannot_be_judged'),
    ('reported-size-ignored-at-load', P, AT_LOAD, '                Some(_) => self.residency.loaded(needed.unwrap_or_default(), true),\n', E, 'memory', 'reports_its_memory'),
    ('size-not-read-again-after-a-run', P, AFTER_RUN, '', E, 'memory', 'grows_between_runs'),
    ('retired-provider-keeps-models', P, '            || retired.load(Ordering::Acquire);', '            || false;', E, 'memory', 'retired_provider_keeps_nothing'),
    ('bad-model-kept-after-a-crash', P, 'let discard = matches!(&result, Err(e) if e.kind == ErrorKind::Internal)\n', 'let discard = false\n', E, 'provider', 'panicking_runner_fails_the_job_and_is_loaded_afresh'),
]

# Group `cm`: the same, through the app's client (`ai-client/src/engines.rs`, `factory.rs`,
# `client.rs`). Unloading one model needs its connection, a replaced engine's models are not
# listed, and a free-memory reader applies whichever order the client and factory are built in.
CE = 'crates/ai-client/src/engines.rs'
CF = 'crates/ai-client/src/factory.rs'
CC = 'crates/ai-client/src/client.rs'
C = 'craft-ai-client'
T = 'loaded_models'

CM = [
    ('unload-one-ignores-connection', CE, '(Some((connection, model)), None) if connection.as_str() == id => {', '(Some((_, model)), None) => {', C, T, 'needs_its_connection'),
    ('label-not-listed', CE, 'connection_label: label.clone(),', 'connection_label: String::new(),', C, T, 'listed_with_its_engine_and_connection'),
    ('unregister-keeps-old-cache', CF, '        self.support.remove(&name);\n        self.engines.remove(&name);\n    }', '        self.support.remove(&name);\n    }', C, T, 'replacing_an_engine_forgets'),
    ('reader-lost-with-a-new-factory', CC, 'if free.is_some() {', 'if false {', C, T, 'whichever_order'),
    ('reader-not-given-to-existing-engines', CE, '            kept.set_free_memory(read.clone());\n', '', C, T, 'after_a_model_loaded'),
    ('replaced-provider-not-retired', CE, '            old.retire();\n', '', C, T, 'changed_mid_job'),
    ('held-retired-provider-dropped', CE, 'Arc::strong_count(p) > 1 || p.is_busy()', 'p.is_busy()', C, T, 'held_across_a_swap'),
    ('loading-job-keeps-model-after-swap', CE, '            old.retire();\n', '            old.unload(None);\n', C, T, 'loading_when_its_connection_changes'),
    ('replaced-provider-hidden', CE, '        merged(out, retiring)\n', '        merged(out, Vec::new())\n', C, T, 'changed_mid_job'),
    ('engines-judge-loads-apart', CE, 'admission: self.load_admission.clone(),', 'admission: LoadAdmission::default(),', C, T, 'two_engines_at_once'),
    ('provider-moved-during-listing-listed-twice', CE, 'if !current.iter().any(|(_, _, p)| Arc::ptr_eq(p, &entry.2)) {', 'if true {', C, 'lib', 'listed_once'),
    ('reader-not-given-to-later-engines', CE, '            kept.set_free_memory(self.free_memory.clone());\n', '', C, T, 'before_its_engines'),
]
