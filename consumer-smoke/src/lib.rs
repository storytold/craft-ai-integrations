//! A separate consumer root for the public `craft-ai-egui` API.
//!
//! This crate is not a member of the repository workspace. Its own `[patch.crates-io]` recipe is
//! the one an adopting app must repeat. It path-depends on this repository's `crates/ai-egui` and
//! `crates/ai-core`, then names the widget-facing core types through `craft_ai_core`; a second
//! `craft-ai-core` package (for example from `craft-libs`) would make that assertion fail to
//! compile. Draw calls use only synthetic data and no host service.

use craft_ai_core::{ConnectionDraft, ConnectionId, DraftError, HistoryLog, JobState};
use craft_ai_egui::{
    ConnectionFormState, HistoryViewState, PanelEvent, connection_form, history_view, job_progress,
};

/// Draws the public widgets in a host-owned `Ui`.
///
/// An adopting app supplies its own fonts and store. This function is compile/runtime smoke only;
/// it starts no provider, download or keychain call.
pub fn draw_public_widgets(
    ui: &mut egui::Ui,
    form: &mut ConnectionFormState,
    history: &mut HistoryViewState,
    log: &HistoryLog,
    now_ms: u64,
) {
    let _ = connection_form(ui, form, false);
    let _ = job_progress(ui, &JobState::Running { percent: Some(25) }, None);
    let _ = history_view(ui, history, log, now_ms);
}

/// The widget-facing values are this workspace's `craft-ai-core` types, not a second identity.
pub fn widget_values_are_the_local_core(form: &ConnectionFormState) {
    let _: &ConnectionDraft = &form.draft;
    let _: &Option<ConnectionId> = &form.draft.editing;
    if let Some(error) = form.error.as_ref() {
        let _: &DraftError = error;
    }
}

/// The panel's saved id is the same core `ConnectionId` an app binds with.
pub fn saved_id(event: PanelEvent) -> Option<ConnectionId> {
    match event {
        PanelEvent::Saved(id) => Some(id),
        PanelEvent::Closed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_widgets_draw_without_a_provider_or_store() {
        let mut form = ConnectionFormState::new();
        let mut history = HistoryViewState::default();
        let log = HistoryLog::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 480.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            draw_public_widgets(ui, &mut form, &mut history, &log, 1_760_000_000_000);
        });
        // This headless API smoke has no renderer; deliberately discard its atlas upload.
        // A native host must apply the texture delta before painting instead.
        output.textures_delta.clear();
        widget_values_are_the_local_core(&form);
    }
}
