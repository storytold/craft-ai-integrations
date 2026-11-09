//! The prices the BFL catalogue carries: FLUX.1 Fill [pro] at its published flat price, and the
//! FLUX 3 models with none, since BFL prices those by resolution tier.

use craft_ai_bfl::BflProvider;
use craft_ai_core::ModelInfo;

/// The catalogue entry with `id`.
fn entry(id: &str) -> ModelInfo {
    BflProvider::catalog()
        .into_iter()
        .find(|m| m.id.as_str() == id)
        .unwrap()
}

/// FLUX.1 Fill [pro] costs 0.05 USD a picture and has no token price.
#[test]
fn fill_carries_its_flat_price() {
    let p = entry("flux-pro-1.0-fill").pricing.unwrap();
    assert_eq!(p.per_image_micros, Some(50_000));
    assert_eq!(
        (p.input_micros_per_mtok, p.output_micros_per_mtok),
        (None, None)
    );
}

/// The FLUX 3 models stay unpriced rather than carrying a guessed tier.
#[test]
fn flux_3_stays_unpriced() {
    assert_eq!(entry("flux-3-image").pricing, None);
    assert_eq!(entry("flux-3-video").pricing, None);
}
