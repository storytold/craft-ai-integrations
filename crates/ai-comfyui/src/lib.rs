//! Offline-testable local ComfyUI workflow adapter. Image/video generation and image inpaint
//! via API-format graphs. No guessed model catalog, large-media handles, or global interruption.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
mod detector;
mod http;
mod input;
mod library;
mod output;
mod provider;
mod sam21_session;
mod sam3_candidates;
mod sam3_detect;
mod sam3_lease;
mod sam3_lifecycle;
mod sam3_masks;
mod sam3_point;
mod sam3_session;
mod scratch;
mod workflow;
pub use detector::FLORENCE2_DETECTOR;
pub use library::{WorkflowLibrary, WorkflowSpec};
pub use provider::{ComfyConfig, ComfyProvider};
pub use sam3_candidates::{Sam3Candidate, Sam3Candidates};
pub use sam3_detect::{Sam3Detections, Sam3Instance};
pub use sam3_lifecycle::{Revision, Sam3Canceller};
pub use sam3_masks::Sam3MaskBatch;
pub use sam3_point::Sam3Point;
pub use sam3_session::Sam3Session;
pub use sam3_session::sam3;
pub use sam21_session::{Sam21Canceller, Sam21Session};
pub use workflow::{Binding, Workflow};
