"""Deliberate-bug cases for group `pf`: files and folders in a person's AI folder stay private to
them. The SAM 3 slot-lock folder (`ai-comfyui/src/sam3_lease.rs`), the folders and history log
`ai-native` creates, the copy of a corrupt store, and the ComfyUI extra-paths file.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter). Most tests are Unix-only, so
the group is meant for the Ubuntu and macOS runs of the Deliberate-bug checks workflow.
"""
LEASE = 'crates/ai-comfyui/src/sam3_lease.rs'
OWNER = 'crates/ai-native/src/owner_only.rs'
HIST = 'crates/ai-native/src/history_file.rs'
SHARED = 'crates/ai-native/src/shared.rs'
FOLDERS = 'crates/ai-client/src/folders.rs'
CU = 'craft-ai-comfyui'
N = 'craft-ai-native'
CL = 'craft-ai-client'

PF = [
    ('lock-folder-in-shared-temp', LEASE, 'Some(dir) => dir.join(LOCK_DIR),', 'Some(_) => temp.join("craft-ai-sam3"),', CU, 'lib', 'the_default_folder_is_in_the_ai_folder'),
    ('lock-fallback-not-per-user', LEASE, '"craft-ai-sam3-{}",', '"craft-ai-sam3{}",', CU, 'lib', 'the_fallback_folder_is_named_for_this_user'),
    ('lock-folder-mode-unchecked', LEASE, '|| meta.mode() & 0o022 != 0', '|| false', CU, 'sam3_leases', 'others_can_write_is_refused'),
    ('lock-folder-link-followed', LEASE, 'let meta = std::fs::symlink_metadata(dir)?;', 'let meta = std::fs::metadata(dir)?;', CU, 'sam3_leases', 'that_is_a_link_is_refused'),
    ('lock-file-link-followed', LEASE, 'options.mode(0o600).custom_flags(libc::O_NOFOLLOW);', 'options.mode(0o600);', CU, 'sam3_leases', 'lock_file_name_is_not_followed'),
    ('lock-file-default-mode', LEASE, 'options.mode(0o600).custom_flags(libc::O_NOFOLLOW);', 'options.custom_flags(libc::O_NOFOLLOW);', CU, 'sam3_leases', 'private_to_the_user'),
    ('native-folders-default-mode', OWNER, '            .mode(0o700)\n', '            .mode(0o755)\n', N, 'lib', 'new_folders_are_private'),
    ('history-created-with-umask', HIST, '            create_private(&self.path)?;\n', '', N, 'lib', 'a_new_history_file_in_an_existing_folder'),
    ('corrupt-copy-keeps-mode', SHARED, 'let _ = fsutil::atomic_copy(&self.path, &PathBuf::from(backup));', 'let _ = std::fs::copy(&self.path, PathBuf::from(backup));', N, 'shared_store', 'copy_of_a_corrupt_store_is_private'),
    ('corrupt-copy-through-link', SHARED, 'let _ = fsutil::atomic_copy(&self.path, &PathBuf::from(backup));', 'let _ = std::fs::copy(&self.path, PathBuf::from(backup));', N, 'shared_store', 'link_at_the_backup_name'),
    ('extra-paths-through-tmp', FOLDERS, 'craft_ai_native::owner_only::write_atomic(file, text.as_bytes()).map_err(io)', '{\n            let mut temp = file.as_os_str().to_owned();\n            temp.push(".tmp");\n            let temp = std::path::PathBuf::from(temp);\n            std::fs::write(&temp, &text).map_err(io)?;\n            std::fs::rename(&temp, file).map_err(io)\n        }', CL, 'folders', 'not_written_through_a_planted_link'),
]
