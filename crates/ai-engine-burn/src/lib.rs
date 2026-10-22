//! SAM 2.1 on the Burn engine for Craft AI: an optional runner for the `image.segment` capability
//! that uses a graphics card when one answers and the processor when none does. The default
//! engine for this model stays rten (`craft-ai-engine-rten`); this one is a choice.
//!
//! An app does not use this crate directly. It puts [`runners`] in an
//! [`EngineRegistry`](craft_ai_engine::EngineRegistry) and asks the client for a capability. See
//! `docs/ADD-AN-ENGINE.md`.
//!
//! - **Same contract as rten**: clicks and a box in, a mask at the picture's size and a `Found`
//!   result out. The request handling is shared with rten, not copied; this crate adds only the
//!   model run. The expensive part, the image encoder, runs once per picture and its result is
//!   kept between requests.
//! - **Weights made on this computer**: the runner reads the saved copy that `craft-ai-burn-setup`
//!   prepared once from the publisher's ONNX files (nothing converted is hosted). Without it the
//!   runner says what to do instead of guessing.
//! - **Never a crash**: weights are loaded through the weights store here, which returns errors,
//!   and every call into the generated model is guarded against panics.
//! - **Generated architecture**: `src/model/` is produced by `tools/burn/generate_sam21.py`. It
//!   is SAM 2.1 tiny only; another model needs its own runner.
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

// The generated code names `alloc::vec::Vec` directly.
extern crate alloc;

mod backend;
// Generated code: its lints are the generator's business, and its forward passes can panic on a
// shape they were not generated for, so the runner calls them only through `backend::guarded`.
#[allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable,
    unused
)]
mod model;
mod runner;

pub use backend::{DECODER_FILE, ENCODER_FILE, graphics_card_answers};
pub use model::ARCHITECTURE_REVISION;
pub use runner::{ENGINE, MODEL, Sam21Runner, runners};
