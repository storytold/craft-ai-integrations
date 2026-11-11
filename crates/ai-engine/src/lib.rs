#![doc = include_str!("../../../docs/ADD-AN-ENGINE.md")]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod job;
mod memory;
mod provider;
mod registry;
mod runner;
pub mod testing;

pub use memory::{FreeMemory, LoadedModelInfo, Unloaded};
pub use provider::EngineProvider;
pub use registry::EngineRegistry;
pub use runner::{LoadedModel, ModelRunner, RunContext};
