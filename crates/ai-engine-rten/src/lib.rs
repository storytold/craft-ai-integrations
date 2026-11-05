//! SAM 2.1 on the rten engine for Craft AI: the built-in runner behind the `image.segment`
//! capability, reading the published ONNX files as they are, for every app that asks for it.
//!
//! An app does not use this crate directly. It puts [`runners`] in an
//! [`EngineRegistry`](craft_ai_engine::EngineRegistry), or turns on the `rten` feature of
//! `craft-ai-client`, and asks the client for a capability. See `docs/ADD-AN-ENGINE.md`.
//!
//! - **Clicks**: positive and negative clicks select one object.
//! - **No conversion**: rten reads the `.onnx` files directly, so the files are the ones the model's
//!   publisher released. Any export with the same inputs and outputs (the larger SAM 2.1 sizes are
//!   drop-in swaps) runs without a code change.
//! - **Every system**: pure Rust on the processor, with SIMD on x86-64 and Arm. No graphics card
//!   is needed or used; a graphics-card engine is a separate entry in the engine list.
//!
//! The expensive part, the image encoder, runs once per picture and the runner keeps its result,
//! so a second click on the same picture is quick.
//!
//! The weights are not part of this repository. The catalogue (`craft-ai-features`) names the
//! exact files, sizes and SHA-256.
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod picture;
pub mod resample;
mod runner;
mod sam21;

// What another engine needs to answer the same request with its own runtime: the request
// handling is [`Loaded`] over a [`Backend`], so only the model run differs.
pub use runner::{ENGINE, Loaded, MODEL, Sam21Runner, read_prompt, runners};
pub use sam21::{
    Area, Backend, Click, DECODER_FILE, ENCODER_FILE, Prediction, best_proposal, prompt_points,
};
