//! Job progress with cancel and a cost line. Works for a two-second image edit (a spinner
//! with no percentage) and a two-minute video job (a bar), and shows the final state afterwards.

use craft_ai_core::{CostEstimate, JobState};
use egui::{Align, InnerResponse, Layout, ProgressBar, TextStyle, Ui};

use crate::text;

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressAction {
    Cancel,
}

/// Label of the cancel button.
pub const CANCEL: &str = "Cancel";

/// Draws the job's state. Returns [`ProgressAction::Cancel`] on the frame the button is pressed;
/// the button only appears while the job can still be cancelled.
pub fn job_progress(
    ui: &mut Ui,
    state: &JobState,
    cost: Option<&CostEstimate>,
) -> Option<ProgressAction> {
    let mut action = None;
    // `ui.vertical` resets the layout direction to left-to-right, so remember the host's
    // direction before entering it and use it for the rows below.
    let right_to_left = ui.layout().prefer_right_to_left();
    ui.vertical(|ui| {
        let label = text::state_label(state);
        let cancel_width = button_width(ui, CANCEL);
        let spacing = ui.spacing().item_spacing.x;
        let available = ui.available_width();
        let cancel = !state.is_terminal();
        let bar_width = if cancel {
            (available - cancel_width - spacing).max(80.0)
        } else {
            available
        };
        let stack = cancel && available - cancel_width - spacing < 80.0;
        match text::progress_fraction(state) {
            Some(f) => {
                let bar = |ui: &mut Ui, width: f32| {
                    ui.add(
                        ProgressBar::new(f)
                            .text(label.clone())
                            .desired_width(width)
                            .animate(false),
                    );
                };
                if stack {
                    bar(ui, available);
                    if ui.button(CANCEL).clicked() {
                        action = Some(ProgressAction::Cancel);
                    }
                } else {
                    row(ui, right_to_left, |ui| {
                        bar(ui, bar_width);
                        if cancel && ui.button(CANCEL).clicked() {
                            action = Some(ProgressAction::Cancel);
                        }
                    });
                }
            }
            None => {
                let label_width = ui
                    .painter()
                    .layout_no_wrap(
                        label.clone(),
                        TextStyle::Body.resolve(ui.style()),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x;
                let spinner_width = if cancel {
                    ui.spacing().interact_size.y + spacing
                } else {
                    0.0
                };
                let needed =
                    spinner_width + label_width + if cancel { spacing + cancel_width } else { 0.0 };
                let stack = cancel && needed > available;
                if stack {
                    if !state.is_terminal() {
                        ui.spinner();
                    }
                    ui.label(label.clone());
                    if ui.button(CANCEL).clicked() {
                        action = Some(ProgressAction::Cancel);
                    }
                } else {
                    row(ui, right_to_left, |ui| {
                        if !state.is_terminal() {
                            ui.spinner();
                        }
                        ui.label(label.clone());
                        if cancel && ui.button(CANCEL).clicked() {
                            action = Some(ProgressAction::Cancel);
                        }
                    });
                }
            }
        }
        if let Some(c) = cost {
            ui.label(text::cost_text(c));
        }
    });
    action
}

/// A horizontal row that follows the host's direction. `Ui::horizontal` would choose from the
/// vertical scope's layout, which loses the right-to-left direction of the host.
fn row<R>(
    ui: &mut Ui,
    right_to_left: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    if right_to_left {
        ui.with_layout(Layout::right_to_left(Align::Center), add_contents)
    } else {
        ui.horizontal(add_contents)
    }
}

/// The width a labelled button needs at this style, including its padding.
fn button_width(ui: &Ui, label: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        TextStyle::Button.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    galley.size().x + 2.0 * ui.spacing().button_padding.x
}
