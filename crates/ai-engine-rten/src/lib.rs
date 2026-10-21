//! The rten engine for Craft AI, first part: the picture handling a selection model needs, with no
//! model in it yet. A later change adds the SAM 2.1 runner on top (`image.segment`).
//!
//! - [`picture`]: a PNG or JPEG becomes RGB bytes (EXIF turn applied, size and memory bounded), and
//!   a mask goes back as a grayscale PNG at the size of the upright source.
//! - [`resample`]: the arithmetic from a picture to the model's input square and from the model's
//!   mask back to the source size.
//!
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
