//! Consent durations exercised through the actual prompt, without a native window or a key.
use craft_ai_core::{AppId, CapabilityId, ConnectionId, ConsentTerm, Disclosure, Locality};
use craft_ai_egui::{
    ConsentChoice, ConsentPromptState, TimedConsentChoice, consent_prompt,
    consent_prompt_with_terms,
};
use egui_kittest::{Harness, kittest::Queryable};

/// What the test host keeps between frames: the disclosure shown, the prompt state and the last
/// answer the prompt returned.
struct State {
    disclosure: Disclosure,
    prompt: ConsentPromptState,
    last: Option<TimedConsentChoice>,
}

/// A kittest harness around the duration-aware prompt, showing a remote disclosure, with nothing
/// answered yet.
fn harness() -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            if let Some(choice) =
                consent_prompt_with_terms(ui, &state.disclosure, "An app", &mut state.prompt)
            {
                state.last = Some(choice);
            }
        },
        State {
            disclosure: Disclosure {
                app: AppId::new("first-app").unwrap(),
                connection: ConnectionId::new("cloud").unwrap(),
                capability: CapabilityId::parse("text.complete").unwrap(),
                locality: Locality::Remote,
                destination: "https://example.com".into(),
                items: vec![],
                param_names: vec![],
            },
            prompt: ConsentPromptState::default(),
            last: None,
        },
    )
}

/// The prompt starts on "This time only", returns nothing until a button is clicked, and Allow
/// then returns that shortest duration.
#[test]
fn allow_defaults_to_this_job_and_nothing_is_agreed_before_clicking() {
    let mut h = harness();
    h.run();
    h.get_by_label("This time only");
    assert_eq!(h.state().last, None);
    h.get_by_label("Allow").click();
    h.run();
    assert_eq!(
        h.state().last,
        Some(TimedConsentChoice::Allow(ConsentTerm::ThisTime))
    );
}

/// Each longer duration, once chosen and allowed, is the duration the prompt returns.
#[test]
fn choosing_a_duration_returns_that_duration() {
    for (label, term) in [
        ("For one hour", ConsentTerm::OneHour),
        ("For one day", ConsentTerm::OneDay),
        ("Until I turn it off", ConsentTerm::UntilRevoked),
    ] {
        let mut h = harness();
        h.run();
        h.get_by_label(label).click();
        h.run();
        h.get_by_label("Allow").click();
        h.run();
        assert_eq!(h.state().last, Some(TimedConsentChoice::Allow(term)));
    }
}

/// Refusing returns Deny even after a long duration was chosen.
#[test]
fn refusing_does_not_return_a_permission() {
    let mut h = harness();
    h.run();
    h.get_by_label("For one day").click();
    h.run();
    h.get_by_label("Don't send").click();
    h.run();
    assert_eq!(h.state().last, Some(TimedConsentChoice::Deny));
}

/// When the disclosure changes after a long duration was chosen, the duration falls back to
/// "This time only".
#[test]
fn a_changed_destination_does_not_inherit_the_longer_duration() {
    let mut h = harness();
    h.run();
    h.get_by_label("Until I turn it off").click();
    h.run();
    h.state_mut().disclosure.destination = "https://another.example.com".into();
    h.run();
    h.get_by_label("Allow").click();
    h.run();
    assert_eq!(
        h.state().last,
        Some(TimedConsentChoice::Allow(ConsentTerm::ThisTime))
    );
}

/// The next prompt starts on "This time only": a long duration chosen for one answer is not kept
/// for the following request.
#[test]
fn a_finished_prompt_does_not_carry_its_duration_to_the_next() {
    let mut h = harness();
    h.run();
    h.get_by_label("Until I turn it off").click();
    h.run();
    h.get_by_label("Allow").click();
    h.run();
    assert_eq!(
        h.state().last,
        Some(TimedConsentChoice::Allow(ConsentTerm::UntilRevoked))
    );
    h.state_mut().last = None;
    h.run();
    h.get_by_label("Allow").click();
    h.run();
    assert_eq!(
        h.state().last,
        Some(TimedConsentChoice::Allow(ConsentTerm::ThisTime))
    );
}

/// The original prompt still offers no durations and returns the plain choice, so hosts that
/// have not moved to the timed prompt see no change.
#[test]
fn the_original_prompt_offers_no_duration() {
    let disclosure = Disclosure {
        app: AppId::new("first-app").unwrap(),
        connection: ConnectionId::new("cloud").unwrap(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        locality: Locality::Remote,
        destination: "https://example.com".into(),
        items: vec![],
        param_names: vec![],
    };
    let mut h = Harness::new_ui_state(
        |ui, last: &mut Option<ConsentChoice>| {
            if let Some(choice) = consent_prompt(ui, &disclosure, "An app") {
                *last = Some(choice);
            }
        },
        None,
    );
    h.run();
    assert!(h.query_by_label("For one hour").is_none());
    assert!(h.query_by_label("This time only").is_none());
    h.get_by_label("Allow").click();
    h.run();
    assert_eq!(*h.state(), Some(ConsentChoice::Allow));
}
