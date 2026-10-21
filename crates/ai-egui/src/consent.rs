//! The consent prompt: what is about to leave the machine, and a clear yes or no.
//!
//! The widget only reports the choice. The app records agreement in its `ConsentLedger` (and saves
//! it) when the choice is [`ConsentChoice::Allow`], and does nothing when it is `Deny`.
//! `AiClient::start` re-checks the ledger regardless, so a prompt that is skipped sends nothing.

use craft_ai_core::{Disclosure, Locality};
use egui::{RichText, Ui};

use crate::text;

/// What the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentChoice {
    Allow,
    Deny,
}

/// The duration attached to a permission. The host applies it with `ConsentLedger::agree`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimedConsentChoice {
    Allow(craft_ai_core::ConsentTerm),
    Deny,
}

/// State belonging to one consent prompt.
pub struct ConsentPromptState {
    term: craft_ai_core::ConsentTerm,
    disclosure: Option<Disclosure>,
}

impl Default for ConsentPromptState {
    /// A fresh prompt: the shortest duration chosen and no disclosure seen yet.
    fn default() -> Self {
        Self {
            term: craft_ai_core::ConsentTerm::ThisTime,
            disclosure: None,
        }
    }
}

/// The duration-aware prompt; the legacy prompt remains available to existing hosts.
pub fn consent_prompt_with_terms(
    ui: &mut Ui,
    disclosure: &Disclosure,
    app_label: &str,
    state: &mut ConsentPromptState,
) -> Option<TimedConsentChoice> {
    if state.disclosure.as_ref() != Some(disclosure) {
        state.term = craft_ai_core::ConsentTerm::ThisTime;
        state.disclosure = Some(disclosure.clone());
    }
    let result =
        draw_consent(ui, disclosure, app_label, Some(&mut state.term)).map(|choice| match choice {
            ConsentChoice::Allow => TimedConsentChoice::Allow(state.term),
            ConsentChoice::Deny => TimedConsentChoice::Deny,
        });
    if result.is_some() {
        *state = ConsentPromptState::default();
    }
    result
}

/// Plain labels for the shared durations; hosts may translate their stable ids instead.
pub fn consent_term_label(term: craft_ai_core::ConsentTerm) -> &'static str {
    use craft_ai_core::ConsentTerm;
    match term {
        ConsentTerm::ThisTime => "This time only",
        ConsentTerm::OneHour => "For one hour",
        ConsentTerm::OneDay => "For one day",
        ConsentTerm::UntilRevoked => "Until I turn it off",
    }
}

/// Label of the button that agrees.
pub const ALLOW: &str = "Allow";
/// Label of the button that refuses.
pub const DENY: &str = "Don't send";

/// Shows the disclosure with a local or remote badge. Returns the choice on the frame it is made.
pub fn consent_prompt(
    ui: &mut Ui,
    disclosure: &Disclosure,
    app_label: &str,
) -> Option<ConsentChoice> {
    draw_consent(ui, disclosure, app_label, None)
}

/// Draws the disclosure and the Allow and Deny buttons. With `term` it also draws one radio per
/// duration, bound to it; without, it draws none. Returns the button pressed on this frame.
fn draw_consent(
    ui: &mut Ui,
    disclosure: &Disclosure,
    app_label: &str,
    term: Option<&mut craft_ai_core::ConsentTerm>,
) -> Option<ConsentChoice> {
    let mut choice = None;
    ui.vertical(|ui| {
        let badge = RichText::new(text::locality_badge(disclosure.locality)).strong();
        let badge = match disclosure.locality {
            Locality::Remote => badge.color(ui.visuals().warn_fg_color),
            Locality::Local => badge,
        };
        ui.label(badge);
        ui.label(text::consent_headline(disclosure, app_label));
        for line in text::disclosure_lines(disclosure) {
            ui.label(format!("\u{2022} {line}"));
        }
        ui.add_space(4.0);
        if let Some(selected) = term {
            ui.label("Allow this for:");
            for duration in craft_ai_core::ConsentTerm::ALL {
                ui.radio_value(selected, duration, consent_term_label(duration));
            }
        }
        ui.horizontal(|ui| {
            if ui.button(ALLOW).clicked() {
                choice = Some(ConsentChoice::Allow);
            }
            if ui.button(DENY).clicked() {
                choice = Some(ConsentChoice::Deny);
            }
        });
    });
    choice
}
