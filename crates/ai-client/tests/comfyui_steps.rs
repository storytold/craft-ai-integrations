//! A workflow imported into the ComfyUI library runs on ComfyUI on this computer, but a step
//! from an add-on can send what it is given anywhere. These tests pin that a workflow using a
//! step outside ComfyUI's own known-local steps is planned as possibly leaving this computer,
//! names those steps, and is not sent until the person has agreed to exactly those steps.
//! A scripted local server stands in for ComfyUI; nothing real is contacted.

mod comfyui_steps_rig;

use comfyui_steps_rig::{World, request, role};
use craft_ai_core::*;

#[test]
fn a_workflow_of_known_local_steps_stays_on_this_computer_and_needs_no_agreement() {
    let w = World::new();
    let plan = w.client.plan(&role(), request()).unwrap();
    assert_eq!(plan.disclosure().locality, Locality::Local);
    assert!(plan.disclosure().unknown_steps.is_empty());
    w.client.start(&plan, &ConsentLedger::default()).unwrap();
    assert!(w.queued());
}

#[test]
fn an_add_on_step_is_named_and_nothing_is_sent_until_the_person_agrees() {
    let w = World::new();
    w.write(&["HttpPost", "HttpPost"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    assert_eq!(plan.disclosure().locality, Locality::Remote);
    assert_eq!(plan.disclosure().unknown_steps, vec!["HttpPost".to_owned()]);
    let e = w
        .client
        .start(&plan, &ConsentLedger::default())
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::ConsentRequired);
    assert!(w.server.recorded().is_empty(), "nothing reaches ComfyUI");
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    w.client.start(&plan, &ledger).unwrap();
    assert!(w.queued());
}

#[test]
fn agreeing_to_some_steps_does_not_cover_a_workflow_that_adds_another() {
    let w = World::new();
    w.write(&["HttpPost"]);
    let mut ledger = ConsentLedger::default();
    ledger.grant(w.client.plan(&role(), request()).unwrap().disclosure());
    w.write(&["HttpPost", "WebhookSend"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    assert_eq!(
        plan.disclosure().unknown_steps,
        vec!["HttpPost".to_owned(), "WebhookSend".to_owned()]
    );
    assert!(plan.needs_consent(&ledger));
    let e = w.client.start(&plan, &ledger).unwrap_err();
    assert_eq!(e.kind, ErrorKind::ConsentRequired);
    assert!(!w.queued());
}

#[test]
fn a_workflow_changed_after_planning_is_not_sent() {
    let w = World::new();
    w.write(&["HttpPost"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    w.write(&["HttpPost", "WebhookSend"]);
    let e = w.client.start(&plan, &ledger).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert!(e.message.contains("workflow changed"), "{}", e.message);
    assert!(!w.queued());
    // A workflow that lost its add-on step after planning is refused the same way.
    let w = World::new();
    w.write(&["HttpPost"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    w.write(&[]);
    assert!(w.client.start(&plan, &ledger).is_err());
    assert!(!w.queued());
}

#[test]
fn a_connection_that_is_not_comfyui_is_not_judged_by_the_workflow_library() {
    // The library is ComfyUI's; a model of another kind that shares a workflow's name is not
    // that workflow.
    let w = World::on("kite-fake");
    w.write(&["HttpPost"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    assert_eq!(plan.disclosure().locality, Locality::Local);
    assert!(plan.disclosure().unknown_steps.is_empty());
}

#[test]
fn steps_whose_names_share_a_long_start_are_agreed_to_separately() {
    // The screen shortens long names; the agreement must not, or consent for one step would
    // cover another whose name only differs after the cut.
    let shared = "A".repeat(80);
    let (one, two) = (format!("{shared}One"), format!("{shared}Two"));
    let w = World::new();
    w.write(&[one.as_str()]);
    let plan = w.client.plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    // A fresh plan for the other step is not covered.
    w.write(&[two.as_str()]);
    let fresh = w.client.plan(&role(), request()).unwrap();
    assert_eq!(fresh.disclosure().unknown_steps, vec![two.clone()]);
    assert!(fresh.needs_consent(&ledger));
    assert_eq!(
        w.client.start(&fresh, &ledger).unwrap_err().kind,
        ErrorKind::ConsentRequired
    );
    // Swapping one for the other after planning is refused too.
    let e = w.client.start(&plan, &ledger).unwrap_err();
    assert!(e.message.contains("workflow changed"), "{}", e.message);
    assert!(!w.queued());
}
