//! Guard: the workspace denies `unsafe_code` (it no longer forbids it), so one named module can
//! allow it: `crates/ai-native/src/owner_only/win_acl.rs`, the Windows access-list calls. This
//! test fails if any other source file loosens that lint, so a new exception has to be named in
//! `AGENTS.md` and added here on purpose rather than slipping in.

use std::fs;
use std::path::{Path, PathBuf};

/// The only file allowed to loosen the lint, relative to the workspace root, with `/`.
const NAMED_EXCEPTION: &str = "crates/ai-native/src/owner_only/win_acl.rs";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn only_the_named_module_allows_unsafe_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(files.len() > 100, "found only {} source files", files.len());

    // Built from parts so this file does not match itself.
    let lint = ["unsafe", "_code"].concat();
    let loosening: Vec<String> = ["allow", "warn", "expect"]
        .iter()
        .map(|level| format!("{level}({lint}"))
        .collect();

    let mut offenders = Vec::new();
    let mut exception_found = false;
    for file in &files {
        let text = fs::read_to_string(file).unwrap_or_default();
        let compact: String = text.split_whitespace().collect();
        if !loosening.iter().any(|l| compact.contains(l.as_str())) {
            continue;
        }
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        if relative == NAMED_EXCEPTION {
            exception_found = true;
        } else {
            offenders.push(relative);
        }
    }
    assert!(
        offenders.is_empty(),
        "unsafe allowed outside the named exception: {offenders:?}"
    );
    assert!(
        exception_found,
        "{NAMED_EXCEPTION} no longer allows unsafe; update this guard"
    );
}
