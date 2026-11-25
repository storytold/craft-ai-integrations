//! InSPyReNet on the Burn engine for `image.matte` (Remove background): the subject's soft alpha
//! at the picture's size, from the picture alone.
//!
//! - **Network**: InSPyReNet with the Swin-B backbone (T. Kim et al., "Revisiting Image Pyramid
//!   Structure for High Resolution Salient Object Detection", ACCV 2022), written by hand in Burn
//!   after the MIT-licensed reference code (`net/`). It reads the published checkpoint as
//!   `.safetensors` and is checked against PyTorch, not generated from an ONNX export.
//! - **Any size**: the network is trained near one megapixel. `pipeline/` runs it at full
//!   resolution up to 1280 pixels on the long side, and above that on a reduced square whose matte
//!   is enlarged back and refined at full resolution with a colour guided filter.
//! - **Answer**: one grayscale PNG at the picture's size, white where the subject is, with soft
//!   values along its edge (hair, fur).

mod fallback;
mod net;
mod pipeline;
mod weights;

use std::path::Path;

use burn::prelude::*;
use burn::tensor::TensorData;
use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, ModelId, ModelInfo, Output,
    Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use craft_ai_engine_rten::picture::{decode_picture, encode_mask};
use craft_ai_features::GpuVendor;

use crate::backend::{pick_device, processor};
use fallback::{WithFallback, load_or_processor};
use net::InSPyReNet;
use net::swin::SwinConfig;
use pipeline::{Input, MAX_RES, Picture, SHRINK};

/// The model id as the catalogue writes it (`NativeRoute.model`).
pub const MODEL: &str = "inspyrenet";

/// The capability this runner answers.
const CAPABILITY: &str = "image.matte";

/// ImageNet channel means and standard deviations: the network's input normalisation.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Stack for building and running the network. Burn recurses deeply through the module tree, more
/// than a default thread has on Windows.
const STACK: usize = 64 << 20;

/// The card makers Burn's graphics layer reaches, as for SAM 2.1 on this engine.
const CARDS: [GpuVendor; 4] = [
    GpuVendor::Nvidia,
    GpuVendor::Amd,
    GpuVendor::Intel,
    GpuVendor::Apple,
];

/// InSPyReNet on the Burn engine. It uses a graphics card when one answers, else the processor.
pub struct InspyrenetRunner {
    /// The backbone size: Swin-B for the published checkpoint, a tiny one in tests.
    backbone: SwinConfig,
    /// Where a load puts the network: the graphics card when one answers, the processor in tests
    /// (a build server's software renderer answers too, and is far too slow for them).
    device: fn() -> Device,
}

impl InspyrenetRunner {
    /// The runner for the published Swin-B checkpoint.
    pub const fn new() -> Self {
        Self {
            backbone: SwinConfig::BASE,
            device: pick_device,
        }
    }
}

impl Default for InspyrenetRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs `work` on a thread with [`STACK`] bytes of stack. A panic in it (Burn and the graphics
/// layer panic on what they cannot handle) comes back as an error a person can read.
fn on_big_stack<T: Send>(work: impl FnOnce() -> Result<T, AiError> + Send) -> Result<T, AiError> {
    let stopped = || {
        AiError::new(
            ErrorKind::Provider,
            "The background remover stopped unexpectedly. Try again, or choose another in Set up AI.",
        )
    };
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("craft-ai-inspyrenet".into())
            .stack_size(STACK)
            .spawn_scoped(scope, work)
            .map_err(|_| stopped())?;
        worker.join().unwrap_or_else(|_| Err(stopped()))
    })
}

impl ModelRunner for InspyrenetRunner {
    fn engine(&self) -> &str {
        crate::runner::ENGINE
    }

    /// `image.matte` from one required picture and nothing else.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "InSPyReNet, graphics card");
        if let Ok(capability) = CapabilityId::parse(CAPABILITY) {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .requiring(InputRole::Source),
            );
        }
        info
    }

    fn accelerators(&self) -> &[GpuVendor] {
        &CARDS
    }

    /// Reads the checkpoint (`inspyrenet.safetensors`) or, in tests, a saved copy (`.bpk`).
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        if !weights.is_file() {
            return Err(AiError::unsupported(
                "The InSPyReNet checkpoint is missing. Open Set up AI to download it.",
            ));
        }
        let backbone = self.backbone;
        let device = (self.device)();
        let (network, on_card) = load_or_processor(
            device != processor(),
            || load_network(backbone, weights, device),
            || load_network(backbone, weights, processor()),
        )?;
        let mut predict: Box<dyn Predict> = Box::new(network);
        if on_card {
            let path = weights.to_path_buf();
            predict = Box::new(WithFallback::new(
                predict,
                Box::new(move || {
                    let network = load_network(backbone, &path, processor())?;
                    Ok(Box::new(network) as Box<dyn Predict>)
                }),
            ));
        }
        Ok(Box::new(Loaded::new(predict)))
    }
}

/// Builds the network on `device` and fills it from the file at `path`, on a thread with a large
/// stack.
fn load_network(backbone: SwinConfig, path: &Path, device: Device) -> Result<Network, AiError> {
    let path = path.to_path_buf();
    on_big_stack(move || {
        let net = weights::fill(InSPyReNet::with_backbone(backbone, &device), &path)?;
        Ok(Network {
            net: Box::new(net),
            device,
        })
    })
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `inspyrenet` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("inspyrenet is a valid model id")
}

/// What the pipeline asks for a picture: the foreground probability of `input`, one value per
/// pixel. The network answers it; tests give a stand-in, as the SAM 2.1 tests here do.
trait Predict: Send {
    fn predict(&mut self, input: &Input, ctx: &RunContext) -> Result<Vec<f32>, AiError>;
}

/// InSPyReNet with its weights, on the device they were loaded to.
struct Network {
    /// Boxed: the module tree is a few hundred kilobytes by value.
    net: Box<InSPyReNet>,
    device: Device,
}

impl Network {
    /// `input` as the network's normalised `[1, 3, H, W]` tensor.
    fn tensor(&self, input: &Input) -> Tensor<4> {
        let (w, h) = (input.width, input.height);
        let mut planar = vec![0.0f32; 3 * w * h];
        for (i, p) in input.px.iter().enumerate() {
            for c in 0..3 {
                if let Some(slot) = planar.get_mut(c * w * h + i) {
                    *slot = (p[c] - MEAN[c]) / STD[c];
                }
            }
        }
        Tensor::from_data(TensorData::new(planar, [1, 3, h, w]), &self.device)
    }
}

impl Predict for Network {
    /// The network's foreground probability for `input`, one value per pixel.
    fn predict(&mut self, input: &Input, ctx: &RunContext) -> Result<Vec<f32>, AiError> {
        let ctx = ctx.clone();
        on_big_stack(|| {
            let cancelled = || ctx.is_cancelled();
            let pred = self
                .net
                .forward(self.tensor(input), &cancelled)
                .ok_or_else(AiError::cancelled)?;
            // A device that fails while running reports it here; `into_data` would panic.
            pred.try_into_data()
                .ok()
                .and_then(|data| data.try_to_vec::<f32>().ok())
                .ok_or_else(|| {
                    AiError::new(
                        ErrorKind::Internal,
                        "The background remover's answer was unreadable.",
                    )
                })
        })
    }
}

/// A loaded model: the any-size pipeline around whatever predicts.
struct Loaded {
    predict: Box<dyn Predict>,
}

impl Loaded {
    fn new(predict: Box<dyn Predict>) -> Self {
        Self { predict }
    }
}

impl LoadedModel for Loaded {
    /// Decodes the picture, runs the any-size pipeline around the network and returns the matte as
    /// a grayscale PNG at the picture's size.
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
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
                "The background remover needs the picture itself, not a link to it.",
            ));
        };
        let rgb = decode_picture(data)?;
        ctx.progress(5);
        ctx.check_cancelled()?;
        let picture = Picture {
            width: rgb.width,
            height: rgb.height,
            bytes: &rgb.bytes,
        };
        ctx.notice(
            "Finding the subject. This takes a few seconds, longer without a graphics card.",
        );
        let mask = pipeline::matte(
            picture,
            MAX_RES,
            SHRINK,
            &mut |input| self.predict.predict(input, ctx),
            ctx,
        )?;
        ctx.check_cancelled()?;
        let alpha: Vec<f32> = mask.iter().map(|v| *v as f32 / 255.0).collect();
        let png = encode_mask(&alpha, rgb.width, rgb.height)?;
        ctx.progress(95);
        Ok(vec![Output {
            payload: Payload::inline("image/png", png)?,
            label: None,
        }])
    }
}

#[cfg(test)]
mod parity;
#[cfg(test)]
mod tests;
