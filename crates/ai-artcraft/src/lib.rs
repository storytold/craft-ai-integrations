//! ArtCraft Omni API-key adapter, separate from account/subscription sign-in.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod fill;
mod http;
mod models;
mod provider;
mod result;
pub use provider::{ArtCraftConfig, ArtCraftProvider};
