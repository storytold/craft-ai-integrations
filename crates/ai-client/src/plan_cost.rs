//! The price a [`Plan`] expects, for an app to show before the person agrees to send.
//!
//! Worked out in [`AiClient::plan`](super::AiClient::plan) from the saved model's pricing by
//! [`craft_ai_core::cost::estimate`]: no key is read and nothing is contacted. `None` means no
//! price is known, never that the job is free.

use craft_ai_core::CostEstimate;

use super::Plan;

impl Plan {
    /// The expected price of this job, or `None` when the saved model has no price that fits it.
    /// Show it beside the disclosure, for example with `craft_ai_egui::text::cost_text`.
    pub fn estimate(&self) -> Option<&CostEstimate> {
        self.estimate.as_ref()
    }
}
