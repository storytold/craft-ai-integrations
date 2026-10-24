//! The add-connection panel: the form plus saving and testing, in one value an app keeps while the
//! panel is open.
//!
//! ```ignore
//! // when ai_status reports StatusAction::AddConnection:
//! panel = Some(ConnectionPanel::new(ConnectionManager::standard()?));
//! // eframe: logic runs even while the window is hidden, ui only when it is drawn.
//! if let Some(p) = &mut panel {
//!     if let Some(event) = p.poll(ctx) {
//!         // Saved is reported by poll; Closed comes from ui below.
//!     }
//! }
//! // every frame while it is open:
//! if let Some(p) = &mut panel {
//!     match p.ui(ui) {
//!         Some(PanelEvent::Saved(_)) | Some(PanelEvent::Closed) => panel = None,
//!         None => {}
//!     }
//! }
//! ```
//!
//! It writes through [`ConnectionManager`], the same code the launcher's connections screen uses,
//! so a connection added here appears in the launcher and in every other Craft app, and the other
//! way round.
//!
//! `ui` only draws and records what was pressed; `poll` starts the test, writes the store and
//! applies the answer. `show` runs both for hosts that drive the panel from one UI callback.

use std::collections::VecDeque;
use std::time::Duration;

use craft_ai_client::{ConnectionManager, SaveError, TestRunner, TestTarget};
use craft_ai_core::{ConnectionDraft, ConnectionId, DraftError, ErrorKind, Field};
use egui::{Context, Ui};

use crate::form::{ConnectionFormState, FormEvent, TestLine, connection_form};

/// What happened in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    /// The connection was saved. The app should look at its status again.
    Saved(ConnectionId),
    /// Cancelled.
    Closed,
}

/// Work the UI asked for. It runs on the next poll, never in the paint callback.
enum PanelCommand {
    StartTest {
        endpoint: String,
        draft: ConnectionDraft,
        generation: u64,
    },
    Save(ConnectionDraft),
}

/// The endpoint and generation a test was started for, so a late answer that no longer describes
/// what the form holds can be dropped.
#[derive(Clone, Debug)]
struct TestCapture {
    endpoint: String,
    generation: u64,
}

/// The fields that decide what a test actually checks. A change to any of them makes an answer
/// that is still on its way stale.
#[derive(PartialEq, Eq)]
struct TestIdentity {
    endpoint: String,
    provider: String,
    key: String,
    uses_key: bool,
    key_header: String,
}

impl TestIdentity {
    fn of(draft: &ConnectionDraft) -> Self {
        Self {
            endpoint: draft.endpoint.clone(),
            provider: draft.provider.clone(),
            key: draft.key.clone(),
            uses_key: draft.uses_key,
            key_header: draft.key_header.clone(),
        }
    }
}

/// The panel's state.
pub struct ConnectionPanel {
    manager: ConnectionManager,
    runner: TestRunner,
    form: ConnectionFormState,
    testing: Option<TestCapture>,
    generation: u64,
    pending: VecDeque<PanelCommand>,
    events: VecDeque<PanelEvent>,
    /// The frame a Save was queued in. A forced second pass receives the same click again; the
    /// monotonic frame number keeps it from queuing a second store write.
    save_frame: Option<u64>,
    /// The frame a Cancel was reported in, so a host that draws twice in one frame cannot close
    /// the panel twice.
    cancel_frame: Option<u64>,
}

/// What one form event did: an event for the host, and whether work is waiting for `poll`.
#[derive(Default)]
struct FormOutcome {
    event: Option<PanelEvent>,
    repaint: bool,
}

impl ConnectionPanel {
    pub fn new(manager: ConnectionManager) -> Self {
        Self::with_form(manager, ConnectionFormState::new())
    }

    /// Starts from a given form, to edit an existing connection.
    pub fn with_form(manager: ConnectionManager, form: ConnectionFormState) -> Self {
        Self {
            runner: TestRunner::new(manager.clone()),
            manager,
            form,
            testing: None,
            generation: 0,
            pending: VecDeque::new(),
            events: VecDeque::new(),
            save_frame: None,
            cancel_frame: None,
        }
    }

    /// Runs the panel's work without drawing it. Call from `App::logic` so a running test still
    /// finishes while the window is hidden, and keep calling `ui` from `App::ui` while open.
    pub fn poll(&mut self, ctx: &Context) -> Option<PanelEvent> {
        if let Some(command) = self.pending.pop_front() {
            match command {
                PanelCommand::StartTest {
                    endpoint,
                    draft,
                    generation,
                } => {
                    let still_current = generation == self.generation
                        && self.testing.as_ref().is_some_and(|capture| {
                            capture.endpoint == endpoint && capture.generation == generation
                        })
                        && self.form.draft.endpoint.trim() == endpoint;
                    if still_current {
                        self.runner.start(TestTarget::Draft(draft));
                    } else if self
                        .testing
                        .as_ref()
                        .is_some_and(|capture| capture.generation == generation)
                    {
                        // The form changed after the click; do not start a test for old values and
                        // do not leave the form showing "Testing" forever.
                        self.cancel_test();
                    }
                }
                PanelCommand::Save(draft) => match self.manager.save(&draft) {
                    Ok(saved) => self.events.push_back(PanelEvent::Saved(saved.id)),
                    Err(SaveError::Invalid(e)) => self.form.error = Some(e),
                    Err(SaveError::Failed(e)) => {
                        self.form.error = Some(DraftError {
                            field: Field::Label,
                            message: e.message,
                        });
                    }
                },
            }
        }

        if self.testing.is_some() {
            if let Some(answer) = self.runner.poll() {
                self.finish_test(answer);
            }
            // Hidden logic still ticks: the integration keeps calling poll until the answer is in.
            if self.testing.is_some() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        self.events.pop_front()
    }

    /// Draws the panel and records what was pressed. Side effects wait for [`Self::poll`].
    pub fn ui(&mut self, ui: &mut Ui) -> Option<PanelEvent> {
        let before = TestIdentity::of(&self.form.draft);
        let event = connection_form(ui, &mut self.form, self.testing.is_some());
        let after = TestIdentity::of(&self.form.draft);
        if before != after {
            self.generation = self.generation.wrapping_add(1);
            if self.testing.is_some() {
                // The running test describes values that are no longer on screen. Drop its answer
                // and let the person start a new test immediately.
                self.cancel_test();
            }
        }
        let outcome = self.handle_form_event(event?, ui.ctx().cumulative_frame_nr());
        if outcome.repaint {
            ui.ctx().request_repaint();
        }
        outcome.event
    }

    /// Applies one press. `frame` is the monotonic frame number, so a host that draws twice in the
    /// same frame cannot queue the same side effect twice.
    fn handle_form_event(&mut self, event: FormEvent, frame: u64) -> FormOutcome {
        let mut outcome = FormOutcome::default();
        match event {
            FormEvent::Cancel => {
                if self.cancel_frame != Some(frame) {
                    self.cancel_frame = Some(frame);
                    outcome.event = Some(PanelEvent::Closed);
                }
            }
            FormEvent::Test => {
                if self.testing.is_none()
                    && !self
                        .pending
                        .iter()
                        .any(|command| matches!(command, PanelCommand::StartTest { .. }))
                {
                    let endpoint = self.form.draft.endpoint.trim().to_owned();
                    self.generation = self.generation.wrapping_add(1);
                    // Set this now, not in poll: a forced second UI pass must see Test disabled
                    // and must not queue a second start.
                    self.form.test = TestLine::Running;
                    self.form.error = None;
                    self.testing = Some(TestCapture {
                        endpoint: endpoint.clone(),
                        generation: self.generation,
                    });
                    self.pending.push_back(PanelCommand::StartTest {
                        endpoint,
                        draft: self.form.draft.clone(),
                        generation: self.generation,
                    });
                    outcome.repaint = true;
                }
            }
            FormEvent::Save => {
                if self.save_frame != Some(frame)
                    && !self
                        .pending
                        .iter()
                        .any(|command| matches!(command, PanelCommand::Save(_)))
                {
                    self.save_frame = Some(frame);
                    self.pending
                        .push_back(PanelCommand::Save(self.form.draft.clone()));
                    outcome.repaint = true;
                }
            }
        }
        outcome
    }

    /// Stops waiting for a test, drops a start that was never admitted, and clears the running line.
    fn cancel_test(&mut self) {
        self.testing = None;
        self.pending
            .retain(|command| !matches!(command, PanelCommand::StartTest { .. }));
        if matches!(self.form.test, TestLine::Running) {
            self.form.test = TestLine::Idle;
        }
    }

    /// Takes the answer only if it still describes the form and the test generation that is
    /// current. This is the guard that keeps a late answer for an old endpoint from replacing the
    /// models the form now shows.
    fn finish_test(&mut self, answer: Result<craft_ai_client::TestReport, craft_ai_core::AiError>) {
        let Some(capture) = self.testing.take() else {
            return;
        };
        if capture.generation == self.generation
            && capture.endpoint == self.form.draft.endpoint.trim()
        {
            self.apply_answer(answer, &capture.endpoint);
        } else {
            self.cancel_test();
        }
    }

    /// Draws the panel and handles what was pressed, for hosts that use one UI callback. Events
    /// keep their order: a save that finishes on the same frame as Cancel is reported before it.
    pub fn show(&mut self, ui: &mut Ui) -> Option<PanelEvent> {
        let polled = self.poll(ui.ctx());
        let drawn = self.ui(ui);
        if let Some(event) = polled {
            self.events.push_front(event);
        }
        if let Some(event) = drawn {
            self.events.push_back(event);
        }
        self.events.pop_front()
    }

    fn apply_answer(
        &mut self,
        answer: Result<craft_ai_client::TestReport, craft_ai_core::AiError>,
        tested: &str,
    ) {
        match answer {
            Ok(report) => {
                let models = report.models.len();
                if self.form.draft.set_models(tested, report.models) {
                    self.form.test = TestLine::Passed {
                        models,
                        millis: u64::try_from(report.elapsed.as_millis()).unwrap_or(u64::MAX),
                    };
                }
            }
            Err(e)
                if e.kind == ErrorKind::Unsupported
                    && self.form.draft.provider == "vertex-express" =>
            {
                self.form.test = TestLine::Unverified(e.message);
            }
            Err(e) => self.form.test = TestLine::Failed(e.message),
        }
    }

    /// The form, for tests and for apps that want to look.
    pub fn form(&self) -> &ConnectionFormState {
        &self.form
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ai_client::TestReport;
    use craft_ai_core::{ModelId, ModelInfo};
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;
    use std::sync::Arc;

    fn panel() -> (tempfile::TempDir, ConnectionPanel) {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let manager = ConnectionManager::new(shared, Arc::new(MemoryStore::new()));
        (dir, ConnectionPanel::new(manager))
    }

    fn answer(model: &str) -> Result<TestReport, craft_ai_core::AiError> {
        Ok(TestReport {
            models: vec![ModelInfo::new(
                ModelId::new(model).unwrap(),
                format!("{model} label"),
            )],
            elapsed: Duration::ZERO,
            probe: None,
        })
    }

    /// A host can draw twice in one frame. The second draw sees the same press and must not queue
    /// a second start, save, or close.
    #[test]
    fn a_second_draw_in_one_frame_does_not_repeat_a_press() {
        let (_dir, mut panel) = panel();

        let _ = panel.handle_form_event(FormEvent::Test, 12);
        let _ = panel.handle_form_event(FormEvent::Test, 12);
        assert_eq!(
            panel
                .pending
                .iter()
                .filter(|command| matches!(command, PanelCommand::StartTest { .. }))
                .count(),
            1,
            "one press, one queued test"
        );
        assert!(panel.testing.is_some(), "Test is disabled immediately");

        assert_eq!(
            panel.handle_form_event(FormEvent::Cancel, 12).event,
            Some(PanelEvent::Closed)
        );
        assert_eq!(
            panel.handle_form_event(FormEvent::Cancel, 12).event,
            None,
            "the same Cancel press is not reported twice"
        );
        assert_eq!(
            panel.handle_form_event(FormEvent::Cancel, 13).event,
            Some(PanelEvent::Closed),
            "a new frame can close"
        );

        let _ = panel.handle_form_event(FormEvent::Save, 14);
        let _ = panel.handle_form_event(FormEvent::Save, 14);
        assert_eq!(
            panel
                .pending
                .iter()
                .filter(|command| matches!(command, PanelCommand::Save(_)))
                .count(),
            1,
            "one press, one queued save"
        );
    }

    /// The panel's own answer guard, exercised without starting a second `TestRunner`. A changed
    /// endpoint and a changed generation each make an answer stale on their own.
    #[test]
    fn a_stale_answer_is_dropped_by_the_panel_guard() {
        for (draft_endpoint, capture_generation) in
            [("http://new/v1", 1u64), ("http://old/v1", 0u64)]
        {
            let (_dir, mut panel) = panel();
            panel.form.draft.endpoint = draft_endpoint.to_owned();
            panel.generation = 1;
            panel.form.test = TestLine::Running;
            panel.testing = Some(TestCapture {
                endpoint: "http://old/v1".to_owned(),
                generation: capture_generation,
            });

            panel.finish_test(answer("old-model"));

            assert!(
                panel.form.draft.models.is_empty(),
                "a stale answer must not supply models: {draft_endpoint}, {capture_generation}"
            );
            assert!(panel.testing.is_none());
            assert!(matches!(panel.form.test, TestLine::Idle));
        }
    }

    /// If a queued start is rejected, the form must not stay on "Testing" with Test disabled.
    #[test]
    fn a_rejected_queued_start_clears_the_running_line() {
        let (_dir, mut panel) = panel();
        panel.form.draft.endpoint = "http://new/v1".to_owned();
        panel.generation = 1;
        panel.form.test = TestLine::Running;
        panel.testing = Some(TestCapture {
            endpoint: "http://old/v1".to_owned(),
            generation: 0,
        });
        panel.pending.push_back(PanelCommand::StartTest {
            endpoint: "http://old/v1".to_owned(),
            draft: panel.form.draft.clone(),
            generation: 0,
        });

        assert!(panel.poll(&Context::default()).is_none());
        assert!(panel.testing.is_none());
        assert!(matches!(panel.form.test, TestLine::Idle));
    }
}
