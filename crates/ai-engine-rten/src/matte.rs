//! BEN2 + ViTMatte on rten as a [`ModelRunner`] for `image.matte` (Remove background): the
//! subject's soft alpha, with every edge pixel matted, as a grayscale PNG the size of the picture.
//!
//! The request is a PNG or JPEG `source` and, optionally, `quality` in params:
//!
//! - `"high"`: the small ViTMatte everywhere, the best for products, lettering and hard-edged
//!   objects;
//! - `"max"` (the default): also the base ViTMatte wherever the edge breaks into strands, for
//!   people with hair.
//!
//! The recipe is in [`pipeline`]. The three files are the publishers' ONNX exports, read as they
//! are: `ben2.onnx` (BEN2 base, MIT), `vitmatte-small.onnx` and `vitmatte-base.onnx` (ViTMatte,
//! Apache-2.0), in one folder.

use std::path::Path;
use std::sync::Arc;

use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, ModelId, ModelInfo, Output,
    Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use rten::Model;
use rten_tensor::NdTensor;
use rten_tensor::prelude::*;

use crate::picture::{decode_picture, encode_mask};
use crate::runner::ENGINE;
use crate::sam21::read_bounded;

pub mod pipeline;
pub mod plane;

pub use pipeline::Quality;

/// The model id as the catalogue writes it (`NativeRoute`).
pub const MODEL: &str = "ben2-vitmatte";
/// The files, in one folder; the weights path the client gives is the first.
pub const BEN2_FILE: &str = "ben2.onnx";
pub const MATTE_SMALL_FILE: &str = "vitmatte-small.onnx";
pub const MATTE_BASE_FILE: &str = "vitmatte-base.onnx";

/// The largest file read: the base ViTMatte is 387 MB.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

const CAPABILITY: &str = "image.matte";

/// BEN2 + ViTMatte on the rten engine. It runs on the processor, on every system.
pub struct MatteRunner;

impl ModelRunner for MatteRunner {
    fn engine(&self) -> &str {
        ENGINE
    }

    /// `image.matte` from one required picture.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "BEN2 + ViTMatte, built in");
        if let Ok(capability) = CapabilityId::parse(CAPABILITY) {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .requiring(InputRole::Source),
            );
        }
        info
    }

    /// Loads the three files when `weights` is `ben2.onnx` and the other two are beside it.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        let dir = weights.parent().filter(|_| {
            weights
                .file_name()
                .is_some_and(|n| n == std::ffi::OsStr::new(BEN2_FILE))
        });
        let Some(dir) = dir.filter(|d| {
            weights.is_file()
                && d.join(MATTE_SMALL_FILE).is_file()
                && d.join(MATTE_BASE_FILE).is_file()
        }) else {
            return Err(AiError::unsupported(
                "BEN2 + ViTMatte needs ben2.onnx, vitmatte-small.onnx and vitmatte-base.onnx together in one folder. Open Set up AI to download them.",
            ));
        };
        Ok(Box::new(Loaded {
            models: Rten::load(dir)?,
        }))
    }
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `ben2-vitmatte` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("ben2-vitmatte is a valid model id")
}

fn failed(what: &str, e: impl std::fmt::Display) -> AiError {
    AiError::new(ErrorKind::Provider, format!("BEN2 + ViTMatte {what}: {e}"))
}

fn not_this_model() -> AiError {
    AiError::unsupported(
        "These files are not the BEN2 + ViTMatte models Craft AI expects. Open Set up AI to download them again.",
    )
}

/// The three graphs loaded by rten.
pub(crate) struct Rten {
    ben2: Model,
    small: Model,
    base: Model,
}

impl Rten {
    fn load(dir: &Path) -> Result<Self, AiError> {
        let read = |file: &str| {
            let bytes = read_bounded(&dir.join(file), MAX_FILE_BYTES).map_err(|e| match e {
                Some(e) => failed("could not be read", e),
                None => not_this_model(),
            })?;
            Model::load(bytes).map_err(|e| failed("could not be loaded", e))
        };
        Ok(Self {
            ben2: read(BEN2_FILE)?,
            small: read(MATTE_SMALL_FILE)?,
            base: read(MATTE_BASE_FILE)?,
        })
    }
}

/// Runs a one-input, one-output graph on a 1 × `c` × `h` × `w` input; its output's values.
fn run(model: &Model, c: usize, h: usize, w: usize, input: Vec<f32>) -> Result<Vec<f32>, AiError> {
    if input.len() != c * h * w {
        return Err(AiError::invalid("A model input of the wrong size."));
    }
    let x = NdTensor::from_data([1, c, h, w], input);
    let out = model
        .run_one(x.view().into(), None)
        .map_err(|e| failed("failed", e))?;
    let t: NdTensor<f32, 4> = out.try_into().map_err(|_| not_this_model())?;
    Ok(t.to_vec())
}

impl pipeline::Models for Rten {
    fn subject(&mut self, input: Vec<f32>) -> Result<Vec<f32>, AiError> {
        run(&self.ben2, 3, pipeline::SIDE, pipeline::SIDE, input)
    }

    fn matte(
        &mut self,
        large: bool,
        input: Vec<f32>,
        h: usize,
        w: usize,
    ) -> Result<Vec<f32>, AiError> {
        run(if large { &self.base } else { &self.small }, 4, h, w, input)
    }
}

/// The quality the request asks for: `max` unless it says `high`.
pub fn read_quality(request: &Request) -> Result<Quality, AiError> {
    match request.params.get("quality") {
        None => Ok(Quality::Max),
        Some(v) => match v.as_str() {
            Some("high") => Ok(Quality::High),
            Some("max") => Ok(Quality::Max),
            _ => Err(AiError::invalid("quality must be \"high\" or \"max\".")),
        },
    }
}

/// The models in memory, behind the recipe.
pub struct Loaded<M: pipeline::Models> {
    models: M,
}

impl<M: pipeline::Models> Loaded<M> {
    /// Wraps a set of models.
    pub fn new(models: M) -> Self {
        Self { models }
    }
}

impl<M: pipeline::Models + Send + 'static> LoadedModel for Loaded<M> {
    /// Decodes the picture, runs the recipe, and returns the matte as a grayscale PNG the size of
    /// the picture as a person sees it.
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
                "BEN2 + ViTMatte needs the picture itself, not a link to it.",
            ));
        };
        let picture = decode_picture(data)?;
        ctx.check_cancelled()?;
        let rgb = pipeline::Rgb::from_bytes(picture.width, picture.height, &picture.bytes);
        drop(picture);
        ctx.notice(
            "Finding the subject, then matting its edge. This takes a while on large pictures.",
        );
        let check = || ctx.check_cancelled();
        let progress = |f: f32| ctx.progress((f.clamp(0.0, 1.0) * 100.0) as u8);
        let ctl = pipeline::Ctl {
            check: &check,
            progress: &progress,
        };
        let alpha = pipeline::cutout(&rgb, &mut self.models, quality, &ctl)?;
        let png = encode_mask(&alpha.v, alpha.w, alpha.h)?;
        Ok(vec![Output {
            payload: Payload::inline("image/png", png)?,
            label: None,
        }])
    }
}

/// This runner, for [`crate::runners`].
pub(crate) fn runner() -> Arc<dyn ModelRunner> {
    Arc::new(MatteRunner)
}

#[cfg(test)]
mod tests;
