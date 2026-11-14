//! Depth Anything V2 Small on Candle as a [`ModelRunner`]: the `image.depth` capability.
//!
//! The request is the shared `image.depth` one: a PNG or JPEG `source` (told apart by its first
//! bytes, a JPEG's EXIF turn applied) carried in the request itself, not as a link. The answer is
//! one 16-bit grayscale PNG the size of the picture as a person sees it, where white is near and
//! black is far. It is a relative map: the values say what is nearer than what, not how many
//! metres. The picture is stretched to 0..65535 from its own nearest to its own farthest part.
//!
//! `quality` (`"high"`, the default, or `"fast"`) picks how it is done:
//!
//! - `fast`: one pass of the network over the whole picture at 518 px on the longer side.
//! - `high`: the same pass, then the picture (at most 2072 px on the longer side, at most 16
//!   tiles) again in overlapping tiles that each get the network's full resolution, every tile
//!   fitted to the whole-picture map and blended in. Slower by about the number of tiles.
//!
//! Both files must sit together in one folder: the depth head (the weights file the connection
//! records) and the DINOv2 backbone it reads. The catalogue (`craft-ai-features`) names them
//! with their sizes and SHA-256. The network code itself is `candle-transformers`'s
//! `depth_anything_v2` and `dinov2`; the picture handling around it is in `prep` and `tiles`.

use std::path::Path;
use std::sync::Arc;

use craft_ai_core::payload::DEFAULT_MAX_INLINE_BYTES;
use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, ModelId, ModelInfo, Output,
    Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};

use crate::image_io::{decode_picture, encode_depth16};
use net::{Estimate, Worker};
use prep::{DepthMap, MODEL_SIZE, check_dims, fit_within, resize_rgb};
use tiles::{MAX_TILES, estimate_tiled, resample_plane_with};

mod net;
mod prep;
#[cfg(test)]
mod tests;
mod tiles;

/// The engine and model ids as the catalogue writes them (`NativeRoute`).
pub(crate) const ENGINE: &str = "candle";
const MODEL: &str = "depth-anything-v2-small";

/// The depth head: the weights file the connection records.
pub const HEAD_FILE: &str = "depth_anything_v2_vits.fp16.safetensors";
/// The DINOv2 backbone, which must sit beside the head.
pub const BACKBONE_FILE: &str = "dinov2_vits14.fp16.safetensors";

/// Longest side of the picture the `high` quality tiles work on.
const HIGH_LONG_SIDE: u32 = 2072;
/// What the network's activations take beyond its weights while a picture runs, about.
const ACTIVATION_BYTES: u64 = 300 * 1024 * 1024;
/// A weights file larger than this is not this model (the real ones are under 50 MB).
const MAX_WEIGHTS_BYTES: u64 = 1024 * 1024 * 1024;

/// An error from the network or the maths around it, in words for the screen.
fn backend(message: impl Into<String>) -> AiError {
    AiError::new(ErrorKind::Provider, message)
}

/// Depth Anything V2 Small on the Candle engine.
pub struct DepthRunner;

impl ModelRunner for DepthRunner {
    /// The engine name on the connection.
    fn engine(&self) -> &str {
        ENGINE
    }

    /// Depth Anything V2 Small, built in, with `image.depth` taking one picture.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "Depth Anything V2 Small, built in");
        if let Ok(capability) = CapabilityId::parse("image.depth") {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .requiring(InputRole::Source),
            );
        }
        info
    }

    /// Loads the depth head from `weights` and the DINOv2 backbone from beside it, or says which
    /// file is missing and where to get it.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        let missing = || {
            AiError::unsupported(format!(
                "Estimate depth needs {HEAD_FILE} and {BACKBONE_FILE} together in one folder. Open Set up AI to download them."
            ))
        };
        let backbone = weights
            .parent()
            .map(|dir| dir.join(BACKBONE_FILE))
            .filter(|b| b.is_file())
            .ok_or_else(missing)?;
        let head = read_weights(weights)?;
        let backbone = read_weights(&backbone)?;
        let net = Worker::start(backbone, head)?;
        Ok(Box::new(Loaded::new(Box::new(net))))
    }

    /// Both files as 32-bit numbers (twice their size, they are half precision), and about 300 MB
    /// for the activations of a pass. The bytes read from the files are dropped once the network
    /// holds its own copy, so they are not counted again.
    fn memory_needed(&self, weights: &Path) -> Option<u64> {
        let size = |p: &Path| std::fs::metadata(p).ok().map(|m| m.len());
        let backbone = weights.parent().map(|d| d.join(BACKBONE_FILE))?;
        let files = size(weights)?.checked_add(size(&backbone)?)?;
        files.checked_mul(2)?.checked_add(ACTIVATION_BYTES)
    }
}

/// Reads one weights file whole, refusing one that cannot be this model by its size.
fn read_weights(path: &Path) -> Result<Vec<u8>, AiError> {
    let unreadable = |_| AiError::unsupported("A depth model file could not be read.");
    let len = std::fs::metadata(path).map_err(unreadable)?.len();
    if len == 0 || len > MAX_WEIGHTS_BYTES {
        return Err(AiError::unsupported(
            "A depth model file is not the file Set up AI downloads. Open Set up AI to download it again.",
        ));
    }
    std::fs::read(path).map_err(unreadable)
}

/// The id the catalogue gives this model.
#[allow(clippy::expect_used)] // the id is a valid literal: it cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("depth-anything-v2-small is a valid model id")
}

/// How much work a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quality {
    Fast,
    High,
}

/// Reads `quality`: `"fast"` or `"high"`, `"high"` when absent, anything else refused.
fn read_quality(request: &Request) -> Result<Quality, AiError> {
    match request.params.get("quality") {
        None => Ok(Quality::High),
        Some(v) => match v.as_str() {
            Some("fast") => Ok(Quality::Fast),
            Some("high") => Ok(Quality::High),
            _ => Err(AiError::invalid("quality must be \"fast\" or \"high\"")),
        },
    }
}

/// The model in memory.
struct Loaded {
    net: Box<dyn Estimate>,
}

impl Loaded {
    fn new(net: Box<dyn Estimate>) -> Self {
        Self { net }
    }

    /// The depth map of an 8-bit RGB picture at the size the quality works at.
    fn map(
        &self,
        picture: &crate::image_io::Rgb,
        quality: Quality,
        ctx: &RunContext,
    ) -> Result<DepthMap, AiError> {
        let size = (picture.width, picture.height);
        let long_side = match quality {
            Quality::Fast => MODEL_SIZE,
            Quality::High => HIGH_LONG_SIDE,
        };
        let (ww, wh) = fit_within(
            u32::try_from(size.0).map_err(|_| crate::image_io::too_large())?,
            u32::try_from(size.1).map_err(|_| crate::image_io::too_large())?,
            long_side,
        );
        check_dims(ww, wh)?;
        let rgb = resize_rgb(
            &picture.bytes,
            size,
            (ww as usize, wh as usize),
            1.0 / 255.0,
        );
        ctx.progress(10);
        ctx.check_cancelled()?;
        let one = |rgb: &[f32], w: u32, h: u32| self.net.estimate(rgb, w, h);
        match quality {
            Quality::Fast => one(&rgb, ww, wh),
            Quality::High => estimate_tiled(&one, &rgb, (ww, wh), MAX_TILES, &|done| {
                ctx.check_cancelled()?;
                ctx.progress(10 + (done * 85.0) as u8);
                Ok(())
            }),
        }
    }
}

impl LoadedModel for Loaded {
    /// Estimates depth for the picture and returns it as one 16-bit grayscale PNG at the
    /// picture's size.
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        let quality = read_quality(request)?;
        let Some(source) = request
            .inputs
            .iter()
            .find(|i| i.role == InputRole::Source)
            .map(|i| &i.payload)
        else {
            return Err(AiError::invalid("A picture is needed."));
        };
        let Payload::Inline { data, .. } = source else {
            return Err(AiError::unsupported(
                "Estimate depth needs the picture itself, not a link to it.",
            ));
        };
        let picture = decode_picture(data)?;
        ctx.progress(5);
        ctx.check_cancelled()?;

        let map = self.map(&picture, quality, ctx)?;
        ctx.progress(95);
        ctx.check_cancelled()?;

        let values = resample_plane_with(
            &map.data,
            (map.width as usize, map.height as usize),
            (picture.width, picture.height),
            |v| (v.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16,
        );
        let png = encode_depth16(&values, picture.width, picture.height)?;
        if png.len() as u64 > DEFAULT_MAX_INLINE_BYTES {
            return Err(AiError::unsupported(
                "The depth map is too large to hand back (over 32 MiB). Use a smaller picture.",
            ));
        }
        Ok(vec![Output {
            payload: Payload::inline("image/png", png)?,
            label: None,
        }])
    }
}

/// Every runner here that is the depth one, for [`crate::runners`].
pub(crate) fn runner() -> Arc<dyn ModelRunner> {
    Arc::new(DepthRunner)
}
