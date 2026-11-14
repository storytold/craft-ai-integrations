//! The two kinds of fill instruction: remove (no prompt) and edit in place (a prompt).

use super::{fill_instruction, fill_instruction_with_references, tidy};

/// An empty or blank prompt asks for removal: the white area is a placeholder to replace.
#[test]
fn no_prompt_removes_what_the_white_area_shows() {
    for blank in ["", "   ", " . ", "\n\t"] {
        let text = fill_instruction(blank);
        assert!(text.contains("only a placeholder"), "{blank:?}: {text}");
        assert!(text.contains("had never existed"), "{blank:?}: {text}");
        assert!(!text.contains("requested change"), "{blank:?}: {text}");
    }
}

/// A prompt asks for an edit in place: only what it names changes, even inside the white area.
#[test]
fn a_prompt_changes_only_what_it_names() {
    let text = fill_instruction("change the eyebrows to bananas");
    assert!(
        text.contains("The requested change is: \"change the eyebrows to bananas\"."),
        "{text}"
    );
    assert!(
        text.contains("Make only the requested change, and only inside the white area."),
        "{text}"
    );
    assert!(text.contains("inside the white area too"), "{text}");
    assert!(text.contains("no visible seam"), "{text}");
    // The real picture is shown, so nothing calls it a placeholder to throw away.
    assert!(!text.contains("placeholder"), "{text}");
    assert!(!text.contains("had never existed"), "{text}");
}

/// The person's words are tidied before they are quoted.
#[test]
fn the_prompt_is_tidied_before_it_is_quoted() {
    assert_eq!(tidy("  a red\n balloon.. "), "a red balloon");
    assert_eq!(tidy("..."), "");
    let text = fill_instruction("  a boat. ");
    assert!(text.contains("\"a boat\""), "{text}");
}

/// References add their guidance after either kind, and none leaves the base text unchanged.
#[test]
fn references_follow_either_kind() {
    assert_eq!(
        fill_instruction_with_references("a hat", 0),
        fill_instruction("a hat")
    );
    let one = fill_instruction_with_references("a hat", 1);
    assert!(one.starts_with(&fill_instruction("a hat")), "{one}");
    assert!(
        one.contains("The third image is a reference picture"),
        "{one}"
    );
    let two = fill_instruction_with_references("", 2);
    assert!(two.starts_with(&fill_instruction("")), "{two}");
    assert!(
        two.contains("Images 3 to 4 are reference pictures"),
        "{two}"
    );
}
