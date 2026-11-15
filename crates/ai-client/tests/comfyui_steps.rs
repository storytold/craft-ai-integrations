//! A workflow imported into the ComfyUI library runs on ComfyUI on this computer, but a step
//! from an add-on can send what it is given anywhere. These tests pin that a workflow using a
//! step outside ComfyUI's own known-local steps is planned as possibly leaving this computer,
//! names those steps, and is not sent until the person has agreed to exactly those steps.
//! A scripted local server stands in for ComfyUI; nothing real is contacted.

mod comfyui_steps_rig;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use comfyui_steps_rig::{World, request, role};
use craft_ai_core::*;
use craft_ai_native::secrets::SecretStore;

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

/// Provider names are case-insensitive in the factory, so every accepted ComfyUI spelling
/// must disclose the same add-on steps and require the same agreement.
#[test]
fn mixed_case_comfyui_names_disclose_add_on_steps() {
    for provider in ["ComfyUI", "COMFYUI"] {
        let w = World::on(provider);
        w.write(&["HttpPost"]);
        let plan = w.client.plan(&role(), request()).unwrap();
        assert_eq!(plan.disclosure().locality, Locality::Remote, "{provider}");
        assert_eq!(
            plan.disclosure().unknown_steps,
            vec!["HttpPost".to_owned()],
            "{provider}"
        );
        assert!(plan.needs_consent(&ConsentLedger::default()), "{provider}");
        assert!(w.server.recorded().is_empty(), "planning sent a request");
    }
}

/// A mixed-case provider reaches the real adapter only after the named add-on steps are
/// agreed to; without agreement not even a metadata request may reach the server.
#[test]
fn mixed_case_comfyui_names_cannot_queue_without_consent() {
    for provider in ["ComfyUI", "COMFYUI"] {
        let w = World::on(provider);
        w.write(&["HttpPost"]);
        let plan = w.client.plan(&role(), request()).unwrap();
        let error = w
            .client
            .start(&plan, &ConsentLedger::default())
            .expect_err("an add-on step needs agreement with every provider spelling");
        assert_eq!(error.kind, ErrorKind::ConsentRequired, "{provider}");
        assert!(
            !w.queued(),
            "an unagreed workflow was queued for {provider}"
        );
        assert!(w.server.recorded().is_empty(), "{provider}");
        let mut ledger = ConsentLedger::default();
        ledger.grant(plan.disclosure());
        w.client.start(&plan, &ledger).unwrap();
        assert!(w.queued(), "the agreed workflow did not run for {provider}");
    }
}

/// A key store that, the moment the key is read, replaces the workflow file with other bytes:
/// the key is read between the step check and building the adapter.
#[derive(Default)]
struct SwapOnRead {
    swap: Mutex<Option<(std::path::PathBuf, Vec<u8>)>>,
    reads: AtomicUsize,
}

impl SecretStore for SwapOnRead {
    /// Identifies this scripted store without naming a real key store.
    fn name(&self) -> &'static str {
        "swap-on-read"
    }
    /// Replaces an armed workflow before returning a fake key and counts every read.
    fn get(&self, _: &SecretRef) -> Result<Option<SecretString>, AiError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some((file, bytes)) = self.swap.lock().unwrap().take() {
            std::fs::write(file, bytes).unwrap();
        }
        Ok(Some(SecretString::new("comfy-key")))
    }
    /// Accepts a write without retaining any key in this test double.
    fn set(&self, _: &SecretRef, _: &SecretString) -> Result<(), AiError> {
        Ok(())
    }
    /// Has no retained key to remove.
    fn delete(&self, _: &SecretRef) -> Result<(), AiError> {
        Ok(())
    }
}

/// A graph imported during key lookup is either refused or excluded from the frozen job.
#[test]
fn a_workflow_replaced_while_the_key_is_read_is_not_sent() {
    let store = Arc::new(SwapOnRead::default());
    let w = World::with("comfyui", Some(store.clone()), |_| {});
    let file = w.dir.path().join("workflows/kite.json");
    // The workflow that will be swapped in adds a step nobody agreed to.
    w.write(&["HttpPost", "WebhookSend"]);
    let swapped = std::fs::read(&file).unwrap();
    w.write(&["HttpPost"]);
    let plan = w.client.plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    *store.swap.lock().unwrap() = Some((file, swapped));
    match w.client.start(&plan, &ledger) {
        Err(error) => {
            assert!(
                error.message.contains("workflow changed"),
                "{}",
                error.message
            );
            assert!(!w.queued(), "the swapped workflow reached ComfyUI");
        }
        Ok(_) => {
            let recorded = w.server.recorded();
            let queued = recorded.iter().find(|r| r.path == "/prompt").unwrap();
            let body = queued.body_json();
            let graph = body["prompt"].as_object().unwrap();
            assert!(
                graph
                    .values()
                    .all(|node| node["class_type"] != "WebhookSend"),
                "an unapproved step reached ComfyUI: {body}"
            );
            assert_eq!(graph.len(), 3, "the checked graph had three steps");
            assert_eq!(graph["10"]["class_type"], "HttpPost");
            assert_eq!(graph["1"]["inputs"]["text"], "a kite");
        }
    }
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
}

/// Registering an app-owned ComfyUI builder retains its override instead of using the default.
#[test]
fn an_app_with_its_own_comfyui_builder_still_builds_with_it() {
    let builds = Arc::new(AtomicUsize::new(0));
    let counted = builds.clone();
    let w = World::with("comfyui", None, move |factory| {
        factory.register("comfyui", move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(AiError::unsupported("the app's own builder"))
        });
    });
    let plan = w.client.plan(&role(), request()).unwrap();
    let e = w
        .client
        .start(&plan, &ConsentLedger::default())
        .unwrap_err();
    assert_eq!(e.message, "the app's own builder");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

/// A damaged selected workflow must be refused before the scripted key store or server is used.
fn assert_workflow_refused_before_key_read(damage: impl FnOnce(&std::path::Path)) {
    let store = Arc::new(SwapOnRead::default());
    let w = World::with("comfyui", Some(store.clone()), |_| {});
    let plan = w.client.plan(&role(), request()).unwrap();
    damage(&w.dir.path().join("workflows/kite.json"));
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    assert!(w.client.start(&plan, &ledger).is_err());
    assert_eq!(store.reads.load(Ordering::SeqCst), 0, "the key was read");
    assert!(w.server.recorded().is_empty(), "a request reached ComfyUI");
}

/// A selected workflow removed after planning has nothing to execute and needs no key read.
#[test]
fn a_missing_workflow_is_refused_before_key_read() {
    assert_workflow_refused_before_key_read(|path| std::fs::remove_file(path).unwrap());
}

/// A non-file at the workflow name cannot be read as a graph and must not reach key lookup.
#[test]
fn an_unreadable_workflow_is_refused_before_key_read() {
    assert_workflow_refused_before_key_read(|path| {
        std::fs::remove_file(path).unwrap();
        std::fs::create_dir(path).unwrap();
    });
}

/// Invalid JSON is skipped by the library but does not authorise reading this job's key.
#[test]
fn an_invalid_workflow_is_refused_before_key_read() {
    assert_workflow_refused_before_key_read(|path| {
        std::fs::write(path, b"not a workflow").unwrap();
    });
}

/// Other valid workflows do not make the selected model available or justify a key read.
#[test]
fn an_absent_selected_workflow_is_refused_before_key_read() {
    assert_workflow_refused_before_key_read(|path| {
        let mut spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        spec["id"] = "another-workflow".into();
        std::fs::write(path, serde_json::to_vec(&spec).unwrap()).unwrap();
    });
}
