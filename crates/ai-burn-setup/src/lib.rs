//! One-time, local preparation of supported Burn weights.
//!
//! The host calls this only after the person chooses this engine and approves setup. Original
//! model files stay untouched. Converted weights belong in the shared model cache, not an app
//! install. This is weight preparation for a model whose Rust architecture ships in the app,
//! not a loader for arbitrary ONNX graphs and not proof that an app can execute the model.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(any(test, feature = "conversion"))]
mod cache;
#[cfg(feature = "conversion")]
mod sam21;

#[cfg(any(test, feature = "conversion"))]
pub use cache::{DAMAGED_COPY, Preparation, PreparedWeights};
#[cfg(feature = "conversion")]
pub use sam21::prepare_sam21;
