"""Deliberate-bug cases for group `kf`: the standard secret chain and its encrypted-file fallback
for a machine with no OS keychain (`ai-native/src/secrets/standard.rs`).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
S = 'crates/ai-native/src/secrets/standard.rs'
N = 'craft-ai-native'

KF = [
    ('locked-file-reads-as-empty', S, '            return Err(self.refuse());\n        }\n        Ok(None)', '            return Ok(None);\n        }\n        Ok(None)', N, 'lib', 'a_locked_file_is_reported'),
    ('locked-file-delete-pretends', S, '            return Err(self.refuse());\n        }\n        Ok(())', '            return Ok(());\n        }\n        Ok(())', N, 'lib', 'a_locked_file_is_reported'),
    ('locked-file-left-out', S, 'None => Box::new(LockedFile { path }),', 'None => {\n                let _ = path;\n                Box::new(EnvStore::with_lookup(|_| None))\n            }', N, 'lib', 'not_hidden_by_a_locked_file'),
    ('empty-passphrase-used', S, 'passphrase.filter(|p| !p.is_empty())', 'passphrase', N, 'lib', 'an_empty_passphrase'),
    ('unlock-skipped-beside-a-keychain', S, 'Some(true) => PassphraseNeed::Unlock,', 'Some(true) if !keychain => PassphraseNeed::Unlock,', N, 'lib', 'passphrase_is_asked_for'),
    ('create-asked-beside-a-keychain', S, 'Some(false) if !keychain => PassphraseNeed::Create,', 'Some(false) => PassphraseNeed::Create,', N, 'lib', 'passphrase_is_asked_for'),
    ('save-refusal-names-no-fix', S, '"this computer has no system keychain, so choose a passphrase to keep keys in an \\\n                 encrypted file",', '"no store",', N, 'lib', 'says_what_to_do'),
]
