//! `cost_text` for the figures a plan's estimate gives: per-image prices that are not whole cents
//! must keep their digits, while whole-cent and sub-cent amounts read as before.

use craft_ai_core::{Billing, CostEstimate};
use craft_ai_egui::text::cost_text;

/// A metered estimate of `micros` millionths of a US dollar.
fn usd(micros: u64) -> CostEstimate {
    CostEstimate {
        micros,
        currency: "USD".into(),
        billing: Billing::Metered,
    }
}

/// A price with a non-zero third place keeps it instead of being cut to two places.
#[test]
fn a_part_cent_price_keeps_its_digits() {
    assert_eq!(cost_text(&usd(39_000)), "about 0.039 USD (billed per use)");
    assert_eq!(cost_text(&usd(156_000)), "about 0.156 USD (billed per use)");
    assert_eq!(
        cost_text(&usd(1_234_500)),
        "about 1.2345 USD (billed per use)"
    );
}

/// Whole-cent, whole-dollar and sub-cent amounts are unchanged.
#[test]
fn other_amounts_read_as_before() {
    assert_eq!(cost_text(&usd(40_000)), "about 0.04 USD (billed per use)");
    assert_eq!(
        cost_text(&usd(2_000_000)),
        "about 2.00 USD (billed per use)"
    );
    assert_eq!(cost_text(&usd(1_500)), "about 0.0015 USD (billed per use)");
}
