//! A price worked out before a job is sent, from the saved model's [`Pricing`] and the request.
//!
//! Nothing is contacted and no key is read: the figures are the ones already in the store.
//! Invariants: a missing price stays `None` and is never guessed. A model that charges by the
//! token gets no estimate, because the number of tokens a reply will use is not known until it
//! comes back. Amounts are whole millionths of a US dollar, as [`Pricing`] stores them, and any
//! overflow gives `None` rather than a wrong figure.

use crate::connection::Billing;
use crate::job::CostEstimate;
use crate::model::Pricing;
use crate::request::Request;

/// The currency every [`Pricing`] figure is in.
const PRICING_CURRENCY: &str = "USD";

/// The expected price of sending `request` to a model priced at `pricing`, billed as `billing`.
///
/// Gives `per_image_micros` times the number of results asked for (`request.outputs`) when the
/// model has a per-result price and no token price above zero. Gives zero when both token prices
/// are stated as zero and there is no per-result price. Gives `None` in every other case: no
/// pricing, a token price above zero, a token price missing beside a zero one, or an overflow.
pub fn estimate(
    pricing: Option<&Pricing>,
    request: &Request,
    billing: Billing,
) -> Option<CostEstimate> {
    let p = pricing?;
    if charges_tokens(p) {
        return None;
    }
    let micros = match p.per_image_micros {
        Some(each) => each.checked_mul(u64::from(request.outputs))?,
        None if p.input_micros_per_mtok == Some(0) && p.output_micros_per_mtok == Some(0) => 0,
        None => return None,
    };
    Some(CostEstimate {
        micros,
        currency: PRICING_CURRENCY.to_owned(),
        billing,
    })
}

/// True when either token price is above zero, so the cost depends on a count not yet known.
fn charges_tokens(p: &Pricing) -> bool {
    p.input_micros_per_mtok.is_some_and(|m| m > 0)
        || p.output_micros_per_mtok.is_some_and(|m| m > 0)
}
