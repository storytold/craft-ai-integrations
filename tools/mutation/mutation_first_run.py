"""Deliberate-bug cases for group `fr`: the first-run setup flow.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
C = 'crates/ai-client/src/'
F = 'crates/ai-features/src/'
CL = 'craft-ai-client'
FE = 'craft-ai-features'
EG = 'craft-ai-egui'

FR = [
    ('agreement-always-asked', F + 'recipe.rs', 'self.models.iter().any(|m| m.licence.accept_required)', 'true', CL, 'first_run', 'asks_for_agreement_only_when'),
    ('download-ignores-a-missing-agreement', C + 'setup_flow.rs', 'if (self.plan.needs_agreement() && !self.accepted)', 'if (false && !self.accepted)', CL, 'first_run', 'asks_for_agreement_only_when'),
    ('start-always-downloads', C + 'setup_flow.rs', 'if existing.is_some() && existing != self.failed_check.as_deref() {\n            SetupTask::CheckExisting', 'if false {\n            SetupTask::CheckExisting', CL, 'first_run', 'start_checks_files'),
    ('suggested-folder-replaces-a-chosen-default', C + 'folders.rs', '.find(|f| f.origin == FolderOrigin::Chosen && f.default))', '.find(|f| f.origin == FolderOrigin::Chosen && f.default && false))', CL, 'first_run', 'suggested_folder_is_made_only_when'),
    ('link-silently-becomes-default', C + 'folders.rs', 'default: choose_if_missing && !store.model_folders.iter().any(|f| f.default),', 'default: !store.model_folders.iter().any(|f| f.default),', CL, 'link_default_safety', 'linked_folder_never_becomes'),
    ('download-falls-back-to-linked', 'crates/ai-core/src/model_folders.rs', '.find(|f| f.origin == FolderOrigin::Chosen && f.default)?;', '.find(|f| f.origin == FolderOrigin::Chosen)?;', CL, 'link_default_safety', 'linked_folder_never_becomes'),
    ('removal-promotes-a-link', C + 'folders.rs', 'store.model_folders.retain(|f| f.id != id);', 'store.model_folders.retain(|f| f.id != id);\n            if let Some(f) = store.model_folders.first_mut() { f.default = true; }', CL, 'link_default_safety', 'linked_folder_never_becomes'),
    ('adopt-forgets-the-kept-choice', C + 'local.rs', '            self.view(&role)?.bound\n', '            None\n', CL, 'first_run', 'adopting_checked_files'),
    ('sample-ignores-the-route', C + 'tools.rs', '.engine_recipe(&route.provider)', '.engine_recipe("not-a-runner")', CL, 'sample_route', 'built_in_route_is_tried'),
    ('nothing-selected-still-passes', C + 'tools.rs', 'is_some_and(|found| !found.matched)', 'is_some_and(|found| false && !found.matched)', CL, 'sample_route', 'chose_nothing'),
    ('status-ignores-a-missing-file', C + 'client.rs', '&& !std::path::Path::new(file).is_file()', '&& false', CL, 'first_run_state', 'file_is_gone_is_not_ready'),
    ('picker-lists-a-gone-model-twice', C + 'picker_groups.rs', 'if files_gone(connection, role, options) {', 'if false {', CL, 'first_run_state', 'file_is_gone_is_not_ready'),
    ('route-note-dropped-from-the-option', F + 'options.rs', 'note: native.note.clone().unwrap_or_default(),', 'note: String::new(),', CL, 'first_run_state', 'clicks_only_route_says_so'),
    ('route-note-dropped-from-the-row', C + 'picker_groups.rs', '.and_then(|n| n.note)', '.and_then(|_| None)', CL, 'first_run_state', 'clicks_only_route_says_so'),
    ('card-engine-outranks-the-default', F + 'engine.rs', 'Some(Pace::Gpu | Pace::Processor) => 0,', 'Some(Pace::Gpu) => 0,\n        Some(Pace::Processor) => 1,', FE, 'engine_default', 'quick_on_the_main_chip'),
    ('quick-engine-still-called-slow', F + 'engine.rs', 'None if offer.fast_on_processor => (', 'None if false && offer.fast_on_processor => (', FE, 'engine_default', 'quick_on_the_main_chip'),
    ('widget-always-shows-the-tick-box', 'crates/ai-egui/src/native_setup.rs', 'if needs_agreement {', 'if true {', EG, 'native_setup_first_run', 'no_licence_to_accept'),
    ('failed-check-is-checked-again', C + 'setup_flow.rs', 'existing.is_some() && existing != self.failed_check.as_deref()', 'existing.is_some()', CL, 'setup_failed_check', 'after_a_failed_check_the_start'),
    ('failed-check-is-forgotten', C + 'setup_flow.rs', 'self.problem = Some(error.kind);\n                if !active.fresh_download {', 'self.problem = Some(error.kind);\n                if false {', CL, 'setup_failed_check', 'after_a_failed_check_the_start'),
    ('failed-recovery-check-is-forgotten', C + 'setup_flow.rs', 'self.problem = Some(error.kind);\n                if !active.fresh_download {', 'self.problem = Some(error.kind);\n                if true {', CL, 'setup_recovery_folder', 'failed_fresh_download'),
    ('failed-download-counts-as-a-failed-check', C + 'setup_flow.rs', '(active.task == SetupTask::CheckExisting)', '(true)', CL, 'setup_failed_check', 'failed_download_does_not'),
    ('chosen-folder-needs-a-suggestion', C + 'folders.rs', 'if let Some(folder) = self.chosen_default()? {\n            return Ok(folder);\n        }\n        let suggested', 'if let Some(folder) = self.chosen_default()?.filter(|_| false) {\n            return Ok(folder);\n        }\n        let suggested', CL, 'lib', 'chosen_folder_needs_no_suggestion'),
    ('widget-picks-its-own-task', 'crates/ai-egui/src/native_setup.rs', 'craft_ai_client::SetupTask::CheckExisting => CHECK_FILES,', 'craft_ai_client::SetupTask::CheckExisting => DOWNLOAD_MODEL,', EG, 'native_setup_one_button', 'one_button_checks_them'),
    ('missing-result-still-passes', C + 'tools.rs', 'if found.is_none() && role.capability.as_str() == segment::CAPABILITY {', 'if false && role.capability.as_str() == segment::CAPABILITY {', CL, 'sample_missing_result', 'no_result_beside_it'),
    ('small-card-calls-a-quick-engine-slow', F + 'engine.rs', 'Some(g) if too_small(g, min_vram_mb) && offer.fast_on_processor => (', 'Some(g) if false && too_small(g, min_vram_mb) && offer.fast_on_processor => (', FE, 'engine_small_card', 'too_small_does_not_make'),
    ('candle-always-reports-a-match', 'crates/ai-engine-candle/src/runner.rs', 'matched: score.is_some(),', 'matched: true,', 'craft-ai-engine-candle', 'lib', 'result_says_whether_anything'),
    ('custom-connection-gets-the-click', C + 'tools.rs', 'route.connection.as_str()\n                        == crate::local::native_connection_id(&feature.id, &route.provider)', 'true', CL, 'sample_route', 'custom_connection_with_a_built_in'),
    ('suggested-folder-path-hidden', 'crates/ai-egui/src/native_setup.rs', 'ui.label(format!("Suggested folder: {}", suggested.to_string_lossy()));', '', EG, 'native_setup_first_run', 'suggested_folder_is_shown'),
    ('suggested-folder-active-while-busy', 'crates/ai-egui/src/native_setup.rs', 'plan.available() && !session.busy()', 'plan.available()', EG, 'native_setup_first_run', 'busy_session_cannot_take'),
    ('recovery-uses-old-folder', C + 'setup_flow.rs', 'self.failed_check.as_ref().is_some_and(|failed| {', 'false && self.failed_check.as_ref().is_some_and(|failed| {', CL, 'setup_recovery_folder', 'bad_same_size_file'),
    ('recovery-uses-preselected-folder', C + 'setup_flow.rs', 'self.failed_destination.as_deref() == Some(destination)', 'false', CL, 'setup_recovery_folder', 'preselected_other_folder'),
    ('recovery-overwrites-intervening-file', C + 'folders/setup.rs', 'std::fs::create_dir(destination)', 'std::fs::create_dir_all(destination)', CL, 'setup_recovery_folder', 'intervening_file'),
    ('missing-folder-reported-as-existing', C + 'folders/setup.rs', '"the chosen model folder is missing; choose a new one"', '"choose a new model folder: this model directory already exists"', CL, 'setup_create_folder_error', 'missing_parent_is_reported'),
]
