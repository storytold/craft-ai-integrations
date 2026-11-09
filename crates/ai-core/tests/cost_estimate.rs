//! `cost::estimate`: the price worked out before sending, from a model's saved pricing alone.
//! Each test pins one rule, so a broken guard fails exactly one of them.

use craft_ai_core::cost::estimate;
use craft_ai_core::{Billing, CapabilityId, CostEstimate, Pricing, Request};

/// An image request asking for `outputs` results.
fn images(outputs: u32) -> Request {
    let mut r = Request::new(CapabilityId::parse("image.generate").unwrap());
    r.outputs = outputs;
    r
}

/// A pricing with only a per-result price.
fn per_image(micros: u64) -> Pricing {
    Pricing {
        per_image_micros: Some(micros),
        ..Pricing::default()
    }
}

/// A per-result price is multiplied by the number of results asked for, in US dollars.
#[test]
fn a_per_image_price_counts_every_result() {
    let got = estimate(Some(&per_image(39_000)), &images(3), Billing::Metered);
    assert_eq!(
        got,
        Some(CostEstimate {
            micros: 117_000,
            currency: "USD".into(),
            billing: Billing::Metered,
        })
    );
}

/// The connection's billing is carried through unchanged.
#[test]
fn the_billing_comes_from_the_caller() {
    let got = estimate(
        Some(&per_image(10)),
        &images(1),
        Billing::SubscriptionAllowance,
    );
    assert_eq!(got.unwrap().billing, Billing::SubscriptionAllowance);
}

/// No saved pricing gives no estimate, never a guessed one.
#[test]
fn no_pricing_gives_none() {
    assert_eq!(estimate(None, &images(1), Billing::Metered), None);
    let empty = Pricing::default();
    assert_eq!(estimate(Some(&empty), &images(1), Billing::Metered), None);
}

/// A token price above zero gives none, even beside a per-result price: the token count is
/// not known until the reply comes back.
#[test]
fn a_token_price_gives_none() {
    let input = Pricing {
        input_micros_per_mtok: Some(150_000),
        ..per_image(40_000)
    };
    let output = Pricing {
        output_micros_per_mtok: Some(600_000),
        ..per_image(40_000)
    };
    assert_eq!(estimate(Some(&input), &images(1), Billing::Metered), None);
    assert_eq!(estimate(Some(&output), &images(1), Billing::Metered), None);
}

/// Both token prices stated as zero is a known price of zero.
#[test]
fn two_zero_token_prices_give_zero() {
    let free = Pricing {
        input_micros_per_mtok: Some(0),
        output_micros_per_mtok: Some(0),
        per_image_micros: None,
    };
    assert_eq!(
        estimate(Some(&free), &images(2), Billing::Metered).map(|c| c.micros),
        Some(0)
    );
}

/// One token price zero and the other unknown is still unknown.
#[test]
fn one_zero_token_price_alone_gives_none() {
    let half = Pricing {
        input_micros_per_mtok: Some(0),
        ..Pricing::default()
    };
    assert_eq!(estimate(Some(&half), &images(1), Billing::Metered), None);
    let other = Pricing {
        output_micros_per_mtok: Some(0),
        ..Pricing::default()
    };
    assert_eq!(estimate(Some(&other), &images(1), Billing::Metered), None);
}

/// A total too large to hold gives none rather than a wrapped figure.
#[test]
fn an_overflowing_total_gives_none() {
    let got = estimate(Some(&per_image(u64::MAX)), &images(2), Billing::Metered);
    assert_eq!(got, None);
}
