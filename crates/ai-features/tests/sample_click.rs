//! The sample for a built-in selection model is a click, not a description.
use craft_ai_core::segment;
use craft_ai_features::{builtin, sample_request, sample_request_for};

/// The sample for a built-in selection model is one click on the disc and no description, so the
/// model that reads clicks only can pass it; the plain case is still the description.
#[test]
fn a_built_in_selection_sample_is_a_click_and_no_description() {
    let select = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let click = sample_request_for(&select, true).unwrap();
    let clicks = segment::clicks(&click).unwrap();
    assert!(clicks.len() == 1 && clicks[0].positive);
    assert!(segment::description(&click).is_none());
    let words = sample_request_for(&select, false).unwrap();
    assert_eq!(words, sample_request(&select).unwrap());
    assert!(segment::description(&words).is_some());
}

/// Only selection changes with a built-in route: every other tool's sample is the plain one.
#[test]
fn only_selection_changes_with_a_built_in_route() {
    for f in builtin().iter().filter(|f| f.id != "select-subject") {
        assert_eq!(
            sample_request_for(f, true),
            sample_request(f),
            "{}: only selection changes with a built-in route",
            f.id
        );
    }
}
