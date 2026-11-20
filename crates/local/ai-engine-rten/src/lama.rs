//! LaMa on rten as a [`ModelRunner`] for `image.inpaint` (Remove objects): fills a marked area
//! from its surroundings without a text prompt and returns a lossless RGB PNG at the size of the
//! upright source picture.
//!
//! The request carries a PNG or JPEG `source` and a PNG `mask` of the same dimensions (white where
//! to remove on an opaque mask, or transparent where to remove when the mask carries alpha).
//!
//! The published ONNX export (`Carve/LaMa-ONNX`, `lama_fp32.onnx`, Apache-2.0, opset 17) takes
//! `image` `[1, 3, 512, 512]` (`0..1`) and `mask` `[1, 1, 512, 512]` (`0` or `1`) and returns
//! `output` `[1, 3, 512, 512]` (`0..255`). [`pipeline`] crops around the marked region with
//! surrounding context, pads or resamples to 512 x 512, and blends across a narrow dilated rim so
//! untouched pixels outside the rim stay identical.

use std::path::Path;
use std::sync::Arc;

use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, Limits, ModelId, ModelInfo,
    Output, Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use rten::{Model, NodeId, Value};
use rten_tensor::{AsView, Tensor};

use crate::picture::decode_picture;
use crate::runner::ENGINE;
use crate::sam21::read_bounded;

pub(crate) mod codec;
pub(crate) mod pipeline;

/// The model id as the catalogue writes it (`NativeRoute`).
pub const MODEL: &str = "lama";
/// The weights file in its folder.
pub const LAMA_FILE: &str = "lama.onnx";

/// Largest model file read: `lama_fp32.onnx` is 208 MB.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

const CAPABILITY: &str = "image.inpaint";

/// LaMa on the rten engine. Runs locally on the processor on every system.
pub struct LamaRunner;

impl ModelRunner for LamaRunner {
    fn engine(&self) -> &str {
        ENGINE
    }

    /// `image.inpaint` requiring `source` and `mask`, with no prompt input so prompt-guided
    /// Generative fill never silently picks a prompt-blind model.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "LaMa, built in");
        if let Ok(capability) = CapabilityId::parse(CAPABILITY) {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .requiring(InputRole::Source)
                    .accepting(InputRole::Mask, 1)
                    .requiring(InputRole::Mask),
            );
        }
        info
    }

    /// Loads `lama.onnx` and checks that its graph has the `image`, `mask` and `output` nodes.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        let named = weights
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new(LAMA_FILE));
        if !named || !weights.is_file() {
            return Err(AiError::unsupported(
                "LaMa needs lama.onnx in its folder. Open Set up AI to download it.",
            ));
        }
        Ok(Box::new(Loaded {
            model: Rten::load(weights)?,
        }))
    }
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `lama` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("lama is a valid model id")
}

fn failed(what: &str, e: impl std::fmt::Display) -> AiError {
    AiError::new(ErrorKind::Provider, format!("LaMa {what}: {e}"))
}

fn not_this_model() -> AiError {
    AiError::unsupported(
        "This file is not the LaMa model Craft AI expects. Open Set up AI to download it again.",
    )
}

/// The ONNX graph loaded by rten, with its three interface node ids verified at load time.
pub(crate) struct Rten {
    model: Model,
    image: NodeId,
    mask: NodeId,
    output: NodeId,
}

impl Rten {
    fn load(weights: &Path) -> Result<Self, AiError> {
        let bytes = Self::read(weights)?;
        let model = Model::load(bytes).map_err(|e| failed("could not be loaded", e))?;
        let id = |name: &str| model.node_id(name).map_err(|_| not_this_model());
        Ok(Self {
            image: id("image")?,
            mask: id("mask")?,
            output: id("output")?,
            model,
        })
    }

    /// Reads `weights` up to [`MAX_FILE_BYTES`]; refuses directories and oversized files unread.
    fn read(weights: &Path) -> Result<Vec<u8>, AiError> {
        read_bounded(weights, MAX_FILE_BYTES).map_err(|e| match e {
            Some(e) => failed("could not be read", e),
            None => not_this_model(),
        })
    }
}

impl pipeline::Inpainter for Rten {
    fn inpaint(&mut self, image: Vec<f32>, mask: Vec<f32>) -> Result<Vec<f32>, AiError> {
        let side = pipeline::SIDE;
        if image.len() != 3 * side * side || mask.len() != side * side {
            return Err(AiError::invalid("A model input of the wrong size."));
        }
        let inputs = vec![
            (
                self.image,
                Value::from(Tensor::from_data(&[1, 3, side, side], image)).into(),
            ),
            (
                self.mask,
                Value::from(Tensor::from_data(&[1, 1, side, side], mask)).into(),
            ),
        ];
        let mut out = self
            .model
            .run(inputs, &[self.output], None)
            .map_err(|e| failed("failed", e))?;
        let Some(first) = out.pop() else {
            return Err(not_this_model());
        };
        let tensor = first.into_tensor::<f32>().ok_or_else(not_this_model)?;
        let values = tensor.to_vec();
        if values.len() != 3 * side * side {
            return Err(not_this_model());
        }
        Ok(values)
    }
}

/// The loaded model in memory, behind [`pipeline`].
pub(crate) struct Loaded<M: pipeline::Inpainter> {
    model: M,
}

impl<M: pipeline::Inpainter> Loaded<M> {
    #[cfg(test)]
    pub(crate) fn new(model: M) -> Self {
        Self { model }
    }
}

impl<M: pipeline::Inpainter + Send + 'static> LoadedModel for Loaded<M> {
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        if !request.params.is_empty() {
            return Err(AiError::unsupported("LaMa takes no options."));
        }
        let source = inline_input(
            request,
            InputRole::Source,
            "A picture is needed.",
            "picture",
        )?;
        let mask_bytes = inline_input(request, InputRole::Mask, "A mask is needed.", "mask")?;
        let picture = decode_picture(source)?;
        let (width, height) = (picture.width, picture.height);
        ctx.progress(5);
        ctx.check_cancelled()?;

        let mask = codec::decode_mask(mask_bytes, width, height)?;
        ctx.progress(10);
        ctx.check_cancelled()?;
        ctx.notice("Filling the marked area from its surroundings.");

        let check = || ctx.check_cancelled();
        let progress = |p: u8| ctx.progress(p);
        let ctl = pipeline::Ctl {
            check: &check,
            progress: &progress,
        };
        let filled = pipeline::remove(picture, &mask, &mut self.model, &ctl)?;
        ctx.check_cancelled()?;
        let png = codec::encode_rgb(&filled, width, height)?;
        drop(filled);
        Ok(vec![answer(png, &Limits::default())?])
    }
}

fn inline_input<'a>(
    request: &'a Request,
    role: InputRole,
    missing_msg: &str,
    noun: &str,
) -> Result<&'a [u8], AiError> {
    let Some(payload) = request
        .inputs
        .iter()
        .find(|i| i.role == role)
        .map(|i| &i.payload)
    else {
        return Err(AiError::invalid(missing_msg));
    };
    match payload {
        Payload::Inline { data, .. } => Ok(data),
        _ => Err(AiError::unsupported(format!(
            "LaMa needs the {noun} itself, not a link to it."
        ))),
    }
}

/// The filled picture as the one output, refused in plain words if it exceeds `limits`.
fn answer(png: Vec<u8>, limits: &Limits) -> Result<Output, AiError> {
    let size = png.len();
    let payload = Payload::inline("image/png", png)?;
    payload.check(limits).map_err(|_| {
        AiError::invalid(format!(
            "The filled picture is {size} bytes, over the {} byte limit for an answer. Try a smaller picture.",
            limits.max_inline_bytes
        ))
    })?;
    Ok(Output {
        payload,
        label: None,
    })
}

/// This runner, for [`crate::runners`].
pub(crate) fn runner() -> Arc<dyn ModelRunner> {
    Arc::new(LamaRunner)
}

#[cfg(test)]
mod tests;
