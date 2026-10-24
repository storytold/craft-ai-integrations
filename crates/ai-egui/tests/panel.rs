//! The add-connection panel driven like a person would: pick, type, test, save. A scripted
//! localhost server stands in for the model server; the secret store is in memory.

#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ai_client::ConnectionManager;
use craft_ai_core::ConnectionId;
use craft_ai_egui::{ConnectionPanel, PanelEvent, TestLine};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use support::{Reply, Server};

const KEY: &str = "sk-panel-key-777";

struct State {
    panel: ConnectionPanel,
    event: Option<PanelEvent>,
    events: Vec<PanelEvent>,
    /// Ask egui to run one more pass in the same frame, so a click's event is seen twice.
    force_multipass: bool,
}

struct Rig {
    h: Harness<'static, State>,
    shared: SharedStore,
    secrets: Arc<MemoryStore>,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    rig_with(false)
}

fn rig_with(force_multipass: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let secrets = Arc::new(MemoryStore::new());
    let manager = ConnectionManager::new(shared.clone(), secrets.clone());
    let h = Harness::new_ui_state(
        |ui, s: &mut State| {
            if s.force_multipass {
                s.force_multipass = false;
                ui.ctx().request_discard("test: force another UI pass");
            }
            if let Some(e) = s.panel.show(ui) {
                s.events.push(e.clone());
                s.event = Some(e);
            }
        },
        State {
            panel: ConnectionPanel::new(manager),
            event: None,
            events: Vec::new(),
            force_multipass,
        },
    );
    Rig {
        h,
        shared,
        secrets,
        _dir: dir,
    }
}

fn models_server() -> Server {
    Server::start(|_| {
        Reply::Json(
            200,
            r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-image-1"}]}"#.into(),
        )
    })
}

fn type_into(h: &mut Harness<'_, State>, role: Role, nth: usize, text: &str) {
    let node = h.get_all_by_role(role).nth(nth).expect("field");
    node.focus();
    node.type_text(text);
    h.run();
}

/// Replaces what is in a text box.
fn replace_text(h: &mut Harness<'_, State>, nth: usize, text: &str) {
    h.get_all_by_role(Role::TextInput)
        .nth(nth)
        .expect("field")
        .focus();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    h.get_all_by_role(Role::TextInput)
        .nth(nth)
        .expect("field")
        .type_text(text);
    h.run();
}

fn wait_until(h: &mut Harness<'_, State>, label: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        h.step();
        if h.query_by_label_contains(label).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("never saw {label:?}");
}

#[test]
fn a_remote_connection_is_saved_with_its_key_in_the_secret_store_only() {
    let mut r = rig();
    r.h.run();
    r.h.get_by_label("Data sent to this connection leaves this computer.");
    type_into(&mut r.h, Role::PasswordInput, 0, KEY);
    r.h.get_by_label("Save").click();
    r.h.run();
    assert_eq!(
        r.h.state().event,
        Some(PanelEvent::Saved(ConnectionId::new("openai").unwrap()))
    );
    let text = std::fs::read_to_string(r.shared.path()).unwrap();
    let loc = r.shared.load().unwrap().connections[0]
        .auth
        .secret_ref()
        .unwrap()
        .clone();
    assert!(loc.as_str().starts_with("openai.key"), "{loc:?}");
    assert!(text.contains(loc.as_str()) && !text.contains(KEY));
    let stored = r.secrets.get(&loc).unwrap().unwrap();
    assert_eq!(stored.expose(), KEY);
}

#[test]
fn a_form_problem_stays_on_screen_and_saves_nothing() {
    let mut r = rig();
    r.h.run();
    r.h.get_by_label("Save").click();
    r.h.run();
    r.h.get_by_role(Role::PasswordInput);
    assert!(r.h.state().event.is_none());
    assert!(r.shared.load().unwrap().connections.is_empty());
    // A key with a space in it is refused, and the message does not repeat it.
    type_into(&mut r.h, Role::PasswordInput, 0, "bad key");
    r.h.get_by_label("Save").click();
    r.h.run();
    r.h.get_by_label_contains("no spaces");
    assert!(r.h.query_by_label_contains("bad key").is_none());
}

#[test]
fn testing_lists_models_before_anything_is_saved() {
    let server = models_server();
    let mut r = rig();
    r.h.run();
    r.h.get_by_role(Role::ComboBox).click();
    r.h.run();
    r.h.get_by_label("Ollama").click();
    r.h.run();
    r.h.get_by_label("Data sent to this connection stays on this computer.");
    replace_text(&mut r.h, 1, &server.url);
    r.h.get_by_label("Test").click();
    // A running test keeps asking for repaints, so `run()` would never settle.
    r.h.step();
    wait_until(&mut r.h, "Connected. Models found: 2");
    assert_eq!(server.count(), 1);
    assert!(
        r.shared.load().unwrap().connections.is_empty(),
        "a test saves nothing"
    );
    // The models it found travel with the saved connection.
    r.h.get_by_label("Save").click();
    r.h.run();
    assert!(matches!(r.h.state().event, Some(PanelEvent::Saved(_))));
    assert_eq!(r.shared.load().unwrap().connections[0].models.len(), 2);
}

#[test]
fn a_dead_server_is_reported_and_editing_the_address_clears_the_old_result() {
    let mut r = rig();
    r.h.run();
    r.h.get_by_role(Role::ComboBox).click();
    r.h.run();
    r.h.get_by_label("Ollama").click();
    r.h.run();
    replace_text(&mut r.h, 1, &Server::dead_url());
    r.h.get_by_label("Test").click();
    // A running test keeps asking for repaints, so `run()` would never settle.
    r.h.step();
    wait_until(&mut r.h, "Test failed");
    replace_text(&mut r.h, 1, "http://127.0.0.1:9/v1");
    assert!(
        r.h.query_by_label_contains("Test failed").is_none(),
        "an old result must not describe a new address"
    );
}

#[test]
fn cancel_closes_without_saving() {
    let mut r = rig();
    r.h.run();
    r.h.get_by_label("Cancel").click();
    r.h.run();
    assert_eq!(r.h.state().event, Some(PanelEvent::Closed));
    assert!(r.shared.load().unwrap().connections.is_empty());
}

#[test]
fn models_found_at_one_address_are_not_saved_for_another() {
    let server = models_server();
    let mut r = rig();
    r.h.run();
    r.h.get_by_role(Role::ComboBox).click();
    r.h.run();
    r.h.get_by_label("Ollama").click();
    r.h.run();
    replace_text(&mut r.h, 1, &server.url);
    r.h.get_by_label("Test").click();
    r.h.step();
    wait_until(&mut r.h, "Connected. Models found: 2");
    // Point it somewhere else after the test, then save: the list belongs to the old address.
    replace_text(&mut r.h, 1, "http://127.0.0.1:9/v1");
    r.h.get_by_label("Save").click();
    r.h.run();
    assert!(matches!(r.h.state().event, Some(PanelEvent::Saved(_))));
    let saved = r.shared.load().unwrap();
    assert_eq!(
        saved.connections[0].endpoint.as_deref(),
        Some("http://127.0.0.1:9/v1")
    );
    assert!(saved.connections[0].models.is_empty());
}

/// Replaces what is in a text box while a test is running: the panel asks for a repaint every
/// frame then, so the `run()` in `replace_text` would never settle.
fn replace_text_mid_test(h: &mut Harness<'_, State>, nth: usize, text: &str) {
    h.get_all_by_role(Role::TextInput)
        .nth(nth)
        .expect("field")
        .focus();
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    h.get_all_by_role(Role::TextInput)
        .nth(nth)
        .expect("field")
        .type_text(text);
    h.step();
}

/// Answers slowly, so the address can be edited while the answer is still on its way.
fn slow_models_server() -> Server {
    Server::start(|_| {
        std::thread::sleep(Duration::from_millis(600));
        Reply::Json(
            200,
            r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-image-1"}]}"#.into(),
        )
    })
}

#[test]
fn models_from_a_test_that_was_still_running_are_not_saved_for_the_new_address() {
    // The sibling test above edits the address *after* the answer is in. This is the other
    // order: the address is edited while the test is still running, so the answer arrives
    // describing an address the form no longer holds.
    let server = slow_models_server();
    let mut r = rig();
    r.h.run();
    r.h.get_by_role(Role::ComboBox).click();
    r.h.run();
    r.h.get_by_label("Ollama").click();
    r.h.run();
    replace_text(&mut r.h, 1, &server.url);
    r.h.get_by_label("Test").click();
    r.h.step();
    replace_text_mid_test(&mut r.h, 1, "http://127.0.0.1:9/v1");
    // Let the answer land and be handled.
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        r.h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.count(), 1, "the test did reach the server");
    assert!(
        r.h.query_by_label_contains("Connected. Models found")
            .is_none(),
        "an answer for an address the form no longer holds is not reported as this one's"
    );
    r.h.get_by_label("Save").click();
    r.h.run();
    assert!(matches!(r.h.state().event, Some(PanelEvent::Saved(_))));
    let saved = r.shared.load().unwrap();
    assert_eq!(
        saved.connections[0].endpoint.as_deref(),
        Some("http://127.0.0.1:9/v1")
    );
    assert!(
        saved.connections[0].models.is_empty(),
        "those models were found at the address that was replaced"
    );
}

fn choose_ollama_and_address(h: &mut Harness<'_, State>, address: &str) {
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label("Ollama").click();
    h.run();
    replace_text(h, 1, address);
}

fn named_models_server(models: &[&str], delay_ms: u64) -> Server {
    let entries: Vec<String> = models
        .iter()
        .map(|model| format!(r#"{{"id":"{model}"}}"#))
        .collect();
    let body = format!(r#"{{"object":"list","data":[{}]}}"#, entries.join(","));
    Server::start(move |_| {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        Reply::Json(200, body.clone())
    })
}

#[test]
fn each_field_has_its_visible_name_and_role_and_the_key_goes_away_when_not_used() {
    let mut r = rig();
    r.h.run();
    let fields: Vec<(Role, Option<String>)> =
        r.h.get_all_by_role(Role::TextInput)
            .map(|node| (node.accesskit_node().role(), node.accesskit_node().label()))
            .collect();
    assert_eq!(
        fields,
        [
            (Role::TextInput, Some("Name".to_owned())),
            (Role::TextInput, Some("Address".to_owned())),
        ]
    );
    let key = r.h.get_by_role(Role::PasswordInput);
    assert_eq!(key.accesskit_node().label().as_deref(), Some("Key"));

    let uses_key = r.h.get_by_label("Uses a key");
    assert_eq!(uses_key.accesskit_node().role(), Role::CheckBox);
    assert_eq!(uses_key.accesskit_node().toggled(), Some(Toggled::True));
    assert!(r.h.unnamed_widgets().is_empty());

    uses_key.click();
    r.h.run();
    assert!(r.h.query_by_role(Role::PasswordInput).is_none());
    assert!(r.h.unnamed_widgets().is_empty());

    for label in ["Save", "Test", "Cancel"] {
        assert_eq!(
            r.h.get_by_label(label).accesskit_node().role(),
            Role::Button,
            "{label}"
        );
    }
}

#[test]
fn test_is_disabled_immediately_and_reports_the_running_state() {
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let server = Server::start(move |_| {
        // Completion is controlled by the test, independent of host scheduling speed.
        let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(10));
        Reply::Json(
            200,
            r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-image-1"}]}"#.into(),
        )
    });
    let mut r = rig();
    r.h.run();
    choose_ollama_and_address(&mut r.h, &server.url);
    r.h.get_by_label("Test").click();
    r.h.step();
    assert!(matches!(r.h.state().panel.form().test, TestLine::Running));
    // The click changes state after the button's current-pass accessibility node is built.
    r.h.step();
    assert!(r.h.get_by_label("Test").accesskit_node().is_disabled());
    release.send(()).unwrap();
    wait_until(&mut r.h, "Connected. Models found: 2");
    assert!(!r.h.get_by_label("Test").accesskit_node().is_disabled());
}

#[test]
fn one_click_tests_and_saves_once_when_egui_runs_a_second_pass() {
    let server = models_server();
    let mut r = rig_with(true);
    r.h.run();
    choose_ollama_and_address(&mut r.h, &server.url);

    r.h.state_mut().force_multipass = true;
    r.h.get_by_label("Test").click();
    r.h.step();
    wait_until(&mut r.h, "Connected. Models found: 2");
    assert_eq!(server.count(), 1, "one click, one test request");
    assert!(
        r.h.state()
            .events
            .iter()
            .all(|event| !matches!(event, PanelEvent::Saved(_))),
        "testing saves nothing"
    );

    r.h.state_mut().force_multipass = true;
    r.h.get_by_label("Save").click();
    r.h.step();
    r.h.step();
    let saved =
        r.h.state()
            .events
            .iter()
            .filter(|event| matches!(event, PanelEvent::Saved(_)))
            .count();
    assert_eq!(saved, 1, "one click, one save event");
    assert_eq!(r.shared.load().unwrap().connections.len(), 1);
}

#[test]
fn a_hidden_poll_finishes_the_test_without_another_ui_pass() {
    let server = models_server();
    let mut r = rig();
    r.h.run();
    choose_ollama_and_address(&mut r.h, &server.url);
    r.h.get_by_label("Test").click();
    r.h.step();
    let passes_before = r.h.ctx.cumulative_pass_nr();
    let ctx = r.h.ctx.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        let state = r.h.state_mut();
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            if let Some(event) = state.panel.poll(ctx) {
                events.push(event);
            }
        });
        if matches!(state.panel.form().test, TestLine::Passed { models: 2, .. }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the hidden poll never got the answer"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        ctx.cumulative_pass_nr(),
        passes_before,
        "run_logic must not run a UI pass"
    );
    assert!(events.is_empty(), "a passing test reports no panel event");
    assert_eq!(server.count(), 1);
}

#[test]
fn a_late_answer_for_the_old_endpoint_cannot_replace_the_new_tests_models() {
    let old = named_models_server(&["old-1", "old-2"], 600);
    let new = named_models_server(&["new-1"], 0);
    let mut r = rig();
    r.h.run();
    choose_ollama_and_address(&mut r.h, &old.url);
    r.h.get_by_label("Test").click();
    r.h.step();

    // Point the form at another endpoint while the first answer is still on its way, then test
    // the new one. The old answer has a different generation and must be dropped.
    replace_text_mid_test(&mut r.h, 1, &new.url);
    assert!(matches!(r.h.state().panel.form().test, TestLine::Idle));
    r.h.get_by_label("Test").click();
    r.h.step();
    wait_until(&mut r.h, "Connected. Models found: 1");

    // Give the slow first server time to answer, then let any stale answer arrive.
    std::thread::sleep(Duration::from_millis(800));
    r.h.step();
    assert!(
        matches!(
            r.h.state().panel.form().test,
            TestLine::Passed { models: 1, .. }
        ),
        "the answer for the replaced endpoint must not win"
    );
    assert_eq!(old.count(), 1);
    assert_eq!(new.count(), 1);

    r.h.get_by_label("Save").click();
    r.h.run();
    let saved = r.shared.load().unwrap();
    assert_eq!(
        saved.connections[0].endpoint.as_deref(),
        Some(new.url.as_str())
    );
    assert_eq!(saved.connections[0].models.len(), 1);
    assert_eq!(saved.connections[0].models[0].id.as_str(), "new-1");
}

#[test]
fn a_save_that_finishes_as_cancel_is_pressed_is_reported_before_closed() {
    let mut r = rig();
    r.h.run();
    type_into(&mut r.h, Role::PasswordInput, 0, KEY);
    r.h.get_by_label("Save").click();
    // Queue the save without giving poll a frame to finish it yet.
    r.h.step();
    r.h.get_by_label("Cancel").click();
    r.h.step();
    assert!(
        matches!(r.h.state().events.first(), Some(PanelEvent::Saved(_))),
        "the save is reported before any close: {:?}",
        r.h.state().events
    );
    r.h.step();
    let events = &r.h.state().events;
    assert_eq!(events.len(), 2, "both events are delivered: {events:?}");
    assert!(matches!(events[0], PanelEvent::Saved(_)));
    assert_eq!(events[1], PanelEvent::Closed);
    assert!(matches!(r.h.state().event, Some(PanelEvent::Closed)));
    assert_eq!(r.shared.load().unwrap().connections.len(), 1);
}
