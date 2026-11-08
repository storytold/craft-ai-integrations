//! The order of `submit`'s refusals around the in-flight limit: a body that cannot fit is refused
//! from its sizes even when the adapter is full, the place is taken before a body is built, and a
//! refusal while building gives the place back.
#![allow(clippy::unwrap_used)]
use super::*;

/// An adapter pointed at a closed port on this computer; nothing in these tests reaches it.
fn provider() -> GeminiProvider {
    GeminiProvider::new(GeminiConfig::new(
        "http://127.0.0.1:9/v1beta",
        SecretString::new("test-key"),
    ))
    .unwrap()
}

/// A fill whose source and mask are `len` zero bytes each: they pass the payload limits but are not
/// pictures, so building the body would fail on the format.
fn fill_of(len: usize) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", vec![0; len]).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", vec![0; 16]).unwrap(),
        )
}

/// The default model's id.
fn model() -> ModelId {
    ModelId::new(DEFAULT_MODEL).unwrap()
}

/// When full, a buildable-looking fill is refused as rate limited, not on its format, so its body
/// was never built; the place count is unchanged.
#[test]
fn a_full_adapter_refuses_before_building_the_body() {
    let provider = provider();
    provider.in_flight.store(MAX_RUNNING, Ordering::SeqCst);
    let err = provider.submit(&model(), &fill_of(16)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited, "{}", err.message);
    assert_eq!(provider.in_flight.load(Ordering::SeqCst), MAX_RUNNING);
}

/// Even when full, a fill too big to send is refused for its size, which needs no place.
#[test]
fn a_fill_too_big_is_refused_for_size_even_when_full() {
    let provider = provider();
    provider.in_flight.store(MAX_RUNNING, Ordering::SeqCst);
    let err = provider.submit(&model(), &fill_of(15_000_001)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest, "{}", err.message);
    assert!(err.message.contains("20 MB"), "{}", err.message);
}

/// With room, the same fill is refused on its format while the body is built, and the place it
/// took is given back.
#[test]
fn a_refusal_while_building_gives_the_place_back() {
    let provider = provider();
    let err = provider.submit(&model(), &fill_of(16)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest, "{}", err.message);
    assert_eq!(provider.in_flight.load(Ordering::SeqCst), 0);
    assert!(lock(&provider.jobs).unwrap().is_empty(), "no job was made");
}
