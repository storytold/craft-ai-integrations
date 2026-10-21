"""Deliberate-bug cases for group `ns`: built-in file discovery, the setup plan and installation.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
All transfers are scripted localhost or tiny cached test files.
Format: (name, file, old, new, package, test target, test filter).
"""
NS = [
    ('native-option-without-engine-is-actionable', 'crates/ai-client/src/picker_groups.rs', 'if client.has_adapter(provider) {', 'if true {', 'craft-ai-client', 'picker_native', 'the_builtin_model_only_offers_setup'),
    ('native-files-mixed-across-directories', 'crates/ai-client/src/folders/native.rs', 'candidates.entry(dir.to_owned())', 'candidates.entry(root.to_owned())', 'craft-ai-client', 'folders', 'native_files_in_separate_directories'),
    ('native-default-allows-linked-folder', 'crates/ai-client/src/folders/native.rs', '.find(|f| f.default && f.origin == FolderOrigin::Chosen)', '.find(|f| f.default)', 'craft-ai-client', 'folders', 'native_downloads_go_only'),
    ('native-unusable-recipe-allowed', 'crates/ai-client/src/folders/native.rs', '.engine_recipe(provider)\n        .and_then(|recipe| {', '.locals\n        .first()\n        .and_then(|recipe| {', 'craft-ai-client', 'native_install', 'an_unusable'),
    ('native-discovery-fails-on-a-gone-folder', 'crates/ai-client/src/folders/native.rs', 'Err(ScanError::NotAFolder) => continue,', 'Err(ScanError::NotAFolder) => return Err(AiError::invalid("gone")),', 'craft-ai-client', 'folders', 'has_gone'),
    ('native-cancel-before-start-ignored', 'crates/ai-client/src/folders/native.rs', 'if request.cancel.load(Ordering::Relaxed) {', 'if false {', 'craft-ai-client', 'native_install', 'cancelled_before_start'),
    ('native-licence-skipped', 'crates/ai-client/src/folders/native.rs', '    if !request.licences_accepted\n        && route\n            .models\n            .iter()\n            .any(|model| model.licence.accept_required)\n    {', '    if false {', 'craft-ai-client', 'native_download', 'declining_a_licence'),
    ('native-cached-install-not-rehashed', 'crates/ai-client/src/folders/native.rs', 'verify_file(&path, model, request.cancel, &mut progress)?;', 'let _ = path;', 'craft-ai-client', 'folders', 'native_install_requires_licence'),
    ('native-existing-files-not-rehashed', 'crates/ai-client/src/folders/native.rs', '        verify_file(&directory.join(&file.filename), file, cancel, &mut |p| {\n            progress(index, p)\n        })?;', '        let _ = (index, file, cancel, &mut *progress);', 'craft-ai-client', 'native_install', 'existing_files_are_hashed'),
    ('native-recipe-filename-can-escape', 'crates/ai-client/src/folders/native.rs', 'if route.models.iter().any(|file| {', 'if false && route.models.iter().any(|file| {', 'craft-ai-client', 'native_install', 'a_recipe_filename_cannot_escape'),
    ('native-scan-lists-any-small-file', 'crates/ai-comfy-env/src/model_folder_scan.rs', '!(is_model_file(&name) || is_known)', '!(is_model_file(&name) || true)', 'craft-ai-comfy-env', 'lib', 'a_known_bundle_file'),
    ('native-scan-known-name-ignores-size', 'crates/ai-comfy-env/src/model_folder_scan.rs', 'k.size == meta.len() && k.filename.eq_ignore_ascii_case(&name)', 'k.filename.eq_ignore_ascii_case(&name)', 'craft-ai-comfy-env', 'lib', 'a_known_bundle_file'),
    ('native-known-bundle-file-counted-unrecognised', 'crates/ai-comfy-env/src/model_folder_scan.rs', '} else if basis.is_none() {', '} else {', 'craft-ai-comfy-env', 'lib', 'a_known_bundle_file'),
    ('native-picker-hides-setup-when-the-file-is-gone', 'crates/ai-client/src/picker_groups.rs', '.is_some_and(|file| Path::new(file).is_file())', '.is_some()', 'craft-ai-client', 'picker_native', 'file_is_gone'),
    ('native-picker-matches-any-connection-of-the-runtime', 'crates/ai-client/src/picker_groups.rs', '&& c.id.as_str() == native_connection_id(feature, provider)', '&& true', 'craft-ai-client', 'picker_native', 'another_model'),
    ('native-picker-ignores-the-model-id', 'crates/ai-client/src/picker_groups.rs', '&& m.id.as_str() == current)', '&& true)', 'craft-ai-client', 'picker_native', 'older_model'),
]
