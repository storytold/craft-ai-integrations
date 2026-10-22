//! Keeps the adoption guides and the code that is tested identical.
//!
//! Every ```rust block in `docs/adopting` must sit under a `<!-- snippet: NAME -->` line, and the
//! same code (after removing its indent) must sit between `// snippet: NAME` and
//! `// end snippet` in `adopting_app_guide.rs` or `adopting_tool_guides.rs`, where it runs. So a
//! reader can trust that what a guide shows has been run. Edit both together, and format the test
//! file with rustfmt first, because the guide shows the formatted code.

use std::collections::BTreeMap;

const GUIDES: &[(&str, &str)] = &[
    (
        "README.md",
        include_str!("../../../docs/adopting/README.md"),
    ),
    (
        "use-in-your-app.md",
        include_str!("../../../docs/adopting/use-in-your-app.md"),
    ),
    (
        "declare-a-tool.md",
        include_str!("../../../docs/adopting/declare-a-tool.md"),
    ),
    (
        "bring-a-service.md",
        include_str!("../../../docs/adopting/bring-a-service.md"),
    ),
    (
        "checklist.md",
        include_str!("../../../docs/adopting/checklist.md"),
    ),
    (
        "troubleshooting.md",
        include_str!("../../../docs/adopting/troubleshooting.md"),
    ),
];

const TESTS: &[(&str, &str)] = &[
    (
        "adopting_app_guide.rs",
        include_str!("adopting_app_guide.rs"),
    ),
    (
        "adopting_tool_guides.rs",
        include_str!("adopting_tool_guides.rs"),
    ),
];

/// Lines of `text` with line endings and trailing spaces removed, so a CRLF checkout compares equal.
fn lines(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim_end().to_owned()).collect()
}

/// Removes the indent shared by every non-blank line.
fn dedent(block: &[String]) -> String {
    let cut = block
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let cut_line = |l: &String| l.get(cut..).unwrap_or("").to_owned();
    block.iter().map(cut_line).collect::<Vec<_>>().join("\n")
}

/// The named blocks in a guide. Panics, naming the file, on a Rust block with no name above it.
fn guide_snippets(file: &str, text: &str) -> Vec<(String, String)> {
    let all = lines(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < all.len() {
        if all[i].starts_with("```rust") {
            panic!(
                "{file} line {}: a Rust block needs a `<!-- snippet: NAME -->` line above it",
                i + 1
            );
        }
        if let Some(name) = all[i]
            .strip_prefix("<!-- snippet: ")
            .and_then(|r| r.strip_suffix(" -->"))
        {
            assert!(
                all.get(i + 1).is_some_and(|l| l.starts_with("```rust")),
                "{file}: snippet {name} is not followed by a Rust block"
            );
            let start = i + 2;
            let end = all[start..]
                .iter()
                .position(|l| l == "```")
                .map(|n| start + n)
                .unwrap_or_else(|| panic!("{file}: snippet {name} is never closed"));
            out.push((name.to_owned(), dedent(&all[start..end])));
            i = end;
        }
        i += 1;
    }
    out
}

/// The tested blocks in a test file.
fn test_snippets(text: &str) -> Vec<(String, String)> {
    let all = lines(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < all.len() {
        if let Some(name) = all[i].trim().strip_prefix("// snippet: ") {
            let start = i + 1;
            let end = all[start..]
                .iter()
                .position(|l| l.trim() == "// end snippet")
                .map(|n| start + n)
                .unwrap_or_else(|| panic!("snippet {name} has no `// end snippet`"));
            out.push((name.to_owned(), dedent(&all[start..end])));
            i = end;
        }
        i += 1;
    }
    out
}

fn collect(
    sources: &[(&str, &str)],
    read: fn(&str, &str) -> Vec<(String, String)>,
) -> BTreeMap<String, (String, String)> {
    let mut map = BTreeMap::new();
    for (file, text) in sources {
        for (name, code) in read(file, text) {
            let clash = map.insert(name.clone(), (file.to_string(), code));
            assert!(clash.is_none(), "snippet name {name} is used twice");
        }
    }
    map
}

#[test]
fn every_guide_block_is_the_code_that_runs() {
    let guides = collect(GUIDES, guide_snippets);
    let tests = collect(TESTS, |_, text| test_snippets(text));
    assert!(!guides.is_empty(), "no snippets found in the guides");

    for (name, (file, shown)) in &guides {
        let (test_file, run) = tests
            .get(name)
            .unwrap_or_else(|| panic!("{file}: snippet {name} has no test that runs it"));
        assert_eq!(
            shown, run,
            "{file}: snippet {name} differs from the code in {test_file}"
        );
    }
    for (name, (test_file, _)) in &tests {
        assert!(
            guides.contains_key(name),
            "{test_file}: snippet {name} is not shown in any guide"
        );
    }
}

#[test]
fn the_comparison_notices_a_changed_block() {
    // Proves the check can fail: the same name with different code must not compare equal.
    let guide = guide_snippets("g.md", "<!-- snippet: a -->\n```rust\nlet x = 1;\n```\n");
    let run = test_snippets("fn a() {\n    // snippet: a\n    let x = 2;\n    // end snippet\n}\n");
    assert_eq!(guide[0].0, run[0].0);
    assert_ne!(guide[0].1, run[0].1);
}
