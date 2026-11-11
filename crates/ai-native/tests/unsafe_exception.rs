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

/// `text` with Rust comments replaced by a space, as the compiler reads them. Strings and
/// character literals are kept whole, so `"//"` inside one does not start a comment.
fn without_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        match (chars[i], next) {
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.push(' ');
            }
            ('/', Some('*')) => {
                // Block comments nest in Rust.
                let mut depth = 0;
                while i < chars.len() {
                    match (chars[i], chars.get(i + 1).copied()) {
                        ('/', Some('*')) => {
                            depth += 1;
                            i += 2;
                        }
                        ('*', Some('/')) => {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => i += 1,
                    }
                }
                out.push(' ');
            }
            ('"', _) => {
                out.push('"');
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        out.push(chars[i]);
                        i += 1;
                    }
                    if let Some(&c) = chars.get(i) {
                        out.push(c);
                    }
                    i += 1;
                }
                if i < chars.len() {
                    out.push('"');
                    i += 1;
                }
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Whether `text` loosens the `unsafe_code` lint anywhere: `allow`, `warn` or `expect`, with the
/// lint in any position of the list, inside `cfg_attr` too.
fn loosens_unsafe(text: &str) -> bool {
    // Built from parts so this file does not match itself.
    let lint = ["unsafe", "_code"].concat();
    let compact: String = without_comments(text).split_whitespace().collect();
    ["allow(", "warn(", "expect("].iter().any(|level| {
        compact.match_indices(level).any(|(at, _)| {
            let args = &compact[at + level.len()..];
            let list = &args[..args.find(')').unwrap_or(args.len())];
            list.split(',').any(|name| name == lint)
        })
    })
}

#[test]
fn the_check_sees_every_way_of_writing_the_attribute() {
    let lint = ["unsafe", "_code"].concat();
    for loose in [
        format!("#![allow({lint})]"),
        format!("#[allow(dead_code, {lint})]"),
        format!("#[allow(\n    dead_code,\n    {lint},\n)]"),
        format!("#[cfg_attr(windows, allow({lint}))]"),
        format!("#![allow(/* reason */ {lint})]"),
        format!("#![allow(/* a /* nested */ note */ dead_code, {lint})]"),
        format!("#![allow(dead_code, // reason\n    {lint})]"),
        format!("#[warn({lint})]"),
        format!("#[expect({lint}, reason = \"x\")]"),
    ] {
        assert!(loosens_unsafe(&loose), "missed: {loose}");
    }
    for strict in [
        format!("#![forbid({lint})]"),
        format!("#![deny({lint})]"),
        format!("#[allow({lint}s_elsewhere)]"),
        "#[allow(dead_code)]".to_owned(),
        format!("// #![allow({lint})] is not allowed here"),
        format!("/* #![allow({lint})] */"),
    ] {
        assert!(!loosens_unsafe(&strict), "false alarm: {strict}");
    }
}

#[test]
fn only_the_named_module_allows_unsafe_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(files.len() > 100, "found only {} source files", files.len());

    let mut offenders = Vec::new();
    let mut exception_found = false;
    for file in &files {
        let text = fs::read_to_string(file).unwrap_or_default();
        if !loosens_unsafe(&text) {
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
