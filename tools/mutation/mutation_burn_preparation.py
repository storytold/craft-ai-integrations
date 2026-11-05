"""bp: local Burn weight preparation, with tiny fake conversions only.

The real converter and tensor parity need the ignored live test and the standalone probe.
The dependency cannot cancel mid-component; the cache must discard a late answer instead.
Case shape: name, file, old, new, package, target, test filter.
"""
FILE = 'crates/ai-burn-setup/src/cache.rs'
PACKAGE = 'craft-ai-burn-setup'


def case(name, old, new, test):
    """Keep the common crate and unit-test target explicit in every resolved case."""
    return name, FILE, old, new, PACKAGE, 'lib', test


BP = [
    case('burn-source-hash-unchecked',
         'copied != file.size || hash_file(&snapshot)? != file.sha256',
         'copied != file.size', 'a_source_hash_mismatch'),
    case('burn-source-size-unchecked',
         'copied != file.size || hash_file(&snapshot)? != file.sha256',
         'hash_file(&snapshot)? != file.sha256', 'a_wrong_source_size'),
    case('burn-saved-hash-unchecked',
         'if hash_file(&final_dir.join(name)).map_err(damaged)? != *expected {',
         'if false {', 'damaged_saved_weights'),
    case('burn-converter-revision-forgotten',
         'digest.update(bundle.revision.as_bytes());',
         '', 'a_new_converter_revision'),
    case('burn-source-identity-forgotten',
         'digest.update(file.sha256.as_bytes());',
         '', 'a_new_expected_source'),
    case('burn-cancel-ignored',
         'if cancelled.load(Ordering::Acquire) {',
         'if false {', 'cancel_'),
    case('burn-empty-weights-accepted',
         'if size == 0 || size > 512 * 1024 * 1024 {',
         'if size > 512 * 1024 * 1024 {', 'empty_converted_weights'),
    case('burn-manifest-cap-ignored',
         'if bytes.len() > 4096 {',
         'if false {', 'an_oversized_manifest'),
    case('burn-source-folder-used-as-cache',
         '&& destination(cache)?.starts_with(original)',
         '&& false', 'the_original_folder_cannot'),
    case('burn-second-conversion-not-locked',
         'lock.try_lock().map_err(|_| {',
         'Ok::<(), std::io::Error>(()).map_err(|_| {', 'another_apps_conversion_lock'),
    case('burn-damaged-manifest-answered-with-try-again',
         'serde_json::from_slice(&bytes).map_err(damaged)?;',
         'serde_json::from_slice(&bytes).map_err(file_error)?;', 'every_kind_of_damaged'),
    case('burn-missing-saved-weight-answered-with-try-again',
         'hash_file(&final_dir.join(name)).map_err(damaged)? != *expected',
         'hash_file(&final_dir.join(name))? != *expected', 'every_kind_of_damaged'),
    case('burn-unopenable-manifest-answered-with-try-again',
         '        File::open(path)\n            .map_err(damaged)?\n            .take(4097)',
         '        File::open(path)\n            .map_err(file_error)?\n            .take(4097)',
         'a_manifest_that_cannot_be_opened'),
    case('burn-other-conversion-answered-with-try-again',
         'return Err(damaged("different conversion"));',
         'return Err(file_error("different conversion"));', 'every_kind_of_damaged'),
    case('burn-killed-run-leftovers-kept',
         '    sweep_stale(cache, &key);\n',
         '', 'leftovers_of_a_killed_run'),
    case('burn-sweep-takes-other-keys',
         '.is_some_and(|n| prefixes.iter().any(|p| n.starts_with(p.as_str())));',
         '.is_some_and(|n| n.contains(".work."));', 'leftovers_of_a_killed_run'),
    case('burn-sweep-takes-a-renamed-copy',
         'SCRATCH_ROLES.map(|role| format!("{key}.{role}."));',
         'SCRATCH_ROLES.map(|_| format!("{key}."));', 'leftovers_of_a_killed_run'),
]
