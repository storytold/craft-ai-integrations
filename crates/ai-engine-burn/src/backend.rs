//! SAM 2.1 as two compiled Burn graphs behind the engine-neutral [`Backend`] seam: an image
//! encoder that runs once per picture and a prompt decoder that runs once per click.
//!
//! The graphs are the generated architecture in `model/`; the weights are the two saved files that
//! `craft-ai-burn-setup` prepared once on this computer. Loading goes through the weights store
//! here, never through the generated `from_file`, because that one panics on a bad file.
//!
//! The generated forward passes can also panic (a shape they were not generated for, a graphics
//! card that goes away). Every call into them is guarded, so the answer to a bad day is an error a
//! person can read, never a crashed app.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

// Burn marks its processor backend as deprecated in favour of `burn-flex`. It is kept because it
// is the one measured against ONNX Runtime (identical masks); swap it when flex has been measured.
#[allow(deprecated)]
use burn::backend::ndarray::NdArrayDevice;
use burn::backend::wgpu::WgpuDevice;
use burn::prelude::*;
use burn::tensor::TensorData;
use burn_store::{ApplyResult, BurnpackStore, ModuleSnapshot};
use craft_ai_core::{AiError, ErrorKind};
use craft_ai_engine_rten::resample::{MASK_SIDE, SIDE};
use craft_ai_engine_rten::{Area, Backend, Click, Prediction, best_proposal, prompt_points};

use crate::model::{decoder, encoder};

/// The two saved weight files, side by side in one folder. The first is the one the connection
/// records.
pub const ENCODER_FILE: &str = "encoder.bpk";
pub const DECODER_FILE: &str = "decoder.bpk";

/// The decoder's mask proposals: one from a single prediction, three from the multi-mask head.
const PROPOSALS: usize = 4;

/// The first bytes of every saved weight file. Checked before anything is allocated, so a file
/// that is not one is refused without waking the graphics card.
const MAGIC: &[u8; 4] = b"NRUB";

/// Runs `work`, turning a panic inside it into `None`.
///
/// The generated model code and the graphics layer under it panic on conditions the runner cannot
/// all foresee. `AssertUnwindSafe` is sound here because nothing a panicking closure touched is
/// used afterwards: the caller returns an error and drops it.
fn guarded<T>(work: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(work)).ok()
}

/// A provider error that says what SAM 2.1 was doing when the runtime failed.
fn failed(what: &str) -> AiError {
    AiError::new(ErrorKind::Provider, format!("SAM 2.1 {what}."))
}

/// Said when a saved file cannot be used: damaged, cut short, or not the model Craft AI expects.
fn unreadable() -> AiError {
    AiError::unsupported(
        "The prepared copy of SAM 2.1 could not be read. Open Set up AI to prepare it again.",
    )
}

/// Whether the file starts like a saved weight file.
fn looks_saved(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == MAGIC
}

/// Whether a graphics card answers a small calculation right now.
///
/// A computer with no usable card panics on the first operation rather than at start-up, so one
/// small operation is tried, guarded. Apps can use this to say plainly which way a model will run.
pub fn graphics_card_answers() -> bool {
    let card: Device = WgpuDevice::default().into();
    let answers = guarded(|| {
        let probe = Tensor::<1>::from_data(TensorData::new(vec![1.0f32, 2.0], [2]), &card);
        probe
            .add_scalar(1.0)
            .into_data()
            .try_to_vec::<f32>()
            .is_ok()
    });
    answers == Some(true)
}

/// Set to `cpu` to make this engine use the processor even when a graphics card answers (to
/// compare the two, or to work around a card that misbehaves).
pub const DEVICE_ENV: &str = "CRAFT_AI_BURN_DEVICE";

/// The graphics card when one answers, else the processor. `CRAFT_AI_BURN_DEVICE=cpu` forces the
/// processor.
pub(crate) fn pick_device() -> Device {
    let forced_processor = std::env::var_os(DEVICE_ENV).is_some_and(|v| v == "cpu");
    if !forced_processor && graphics_card_answers() {
        return WgpuDevice::default().into();
    }
    processor()
}

/// The processor, through Burn's `ndarray` backend.
pub(crate) fn processor() -> Device {
    #[allow(deprecated)]
    let cpu = NdArrayDevice::Cpu;
    cpu.into()
}

/// Whether every weight of the model was set, with no error on the way. The store already refuses
/// both with its default settings; this keeps a changed setting or a newer store from letting a
/// half-loaded model run.
fn fully_applied(result: &ApplyResult) -> bool {
    result.is_success() && result.missing.is_empty()
}

/// Fills `model` from the saved file at `path`, refusing a file that is damaged, belongs to
/// another model or leaves any weight unset.
fn fill<M: ModuleSnapshot>(mut model: M, path: &Path) -> Result<M, AiError> {
    let mut store = BurnpackStore::from_file(path);
    match guarded(|| model.load_from(&mut store)) {
        Some(Ok(applied)) if fully_applied(&applied) => Ok(model),
        _ => Err(unreadable()),
    }
}

/// What the encoder leaves behind: the three tensors the decoder reads, kept on the device.
pub(crate) struct Features {
    embed: Tensor<4>,
    high_res_0: Tensor<4>,
    high_res_1: Tensor<4>,
}

/// The real backend: both graphs with their weights, and the device they run on.
pub(crate) struct Burn {
    device: Device,
    encoder: encoder::Model,
    decoder: decoder::Model,
}

impl Burn {
    /// Loads `encoder.bpk` and `decoder.bpk` from `dir`. Says what is wrong with a file that is
    /// missing, is not a saved weight file or does not fit the model.
    pub(crate) fn load(dir: &Path) -> Result<Self, AiError> {
        let encoder_file = dir.join(ENCODER_FILE);
        let decoder_file = dir.join(DECODER_FILE);
        if !looks_saved(&encoder_file) || !looks_saved(&decoder_file) {
            return Err(unreadable());
        }
        let device = pick_device();
        let encoder = guarded(|| encoder::Model::new(&device)).ok_or_else(unreadable)?;
        let decoder = guarded(|| decoder::Model::new(&device)).ok_or_else(unreadable)?;
        Ok(Self {
            encoder: fill(encoder, &encoder_file)?,
            decoder: fill(decoder, &decoder_file)?,
            device,
        })
    }
}

/// The best proposal out of the decoder's masks and scores, once both have the length the model
/// promises. Anything else means these are not the files this runner was built for.
pub(crate) fn choose_proposal(masks: &[f32], scores: &[f32]) -> Result<Prediction, AiError> {
    let side = MASK_SIDE * MASK_SIDE;
    if masks.len() != PROPOSALS * side || scores.len() != PROPOSALS {
        return Err(unreadable());
    }
    let best = best_proposal(scores);
    let logits = masks
        .get(best * side..(best + 1) * side)
        .ok_or_else(unreadable)?
        .to_vec();
    Ok(Prediction {
        logits,
        score: scores.get(best).copied().unwrap_or(0.0),
    })
}

impl Backend for Burn {
    type Features = Features;

    /// Runs the encoder on the prepared picture and waits for it, so a failure shows here and not
    /// at the next click.
    fn encode(&self, picture: Vec<f32>) -> Result<Features, AiError> {
        if picture.len() != 3 * SIDE * SIDE {
            return Err(failed("was given a picture of the wrong size"));
        }
        guarded(|| {
            let image =
                Tensor::<4>::from_data(TensorData::new(picture, [1, 3, SIDE, SIDE]), &self.device);
            let (embed, high_res_0, high_res_1) = self.encoder.forward(image);
            // Reading a result is what waits for the card to finish.
            let done = embed.clone().into_data().try_to_vec::<f32>().is_ok();
            done.then_some(Features {
                embed,
                high_res_0,
                high_res_1,
            })
        })
        .flatten()
        .ok_or_else(|| failed("could not look at the picture"))
    }

    /// Runs the decoder on the clicks and, after them, the box as its two corner points, and keeps
    /// the proposal the model rates highest.
    fn segment(
        &self,
        features: &Features,
        clicks: &[Click],
        area: Option<Area>,
    ) -> Result<Prediction, AiError> {
        let (coords, labels) = prompt_points(clicks, area);
        let n = labels.len();
        if n == 0 || coords.len() != n * 2 {
            return Err(AiError::invalid(
                "Click on what you want to select, or draw a box around it.",
            ));
        }
        let labels: Vec<i64> = labels.into_iter().map(i64::from).collect();
        let (masks, scores) = guarded(|| {
            let points = Tensor::<3>::from_data(TensorData::new(coords, [1, n, 2]), &self.device);
            let labels = Tensor::<2, Int>::from_data(TensorData::new(labels, [1, n]), &self.device);
            let (masks, iou) = self.decoder.forward(
                features.embed.clone(),
                features.high_res_0.clone(),
                features.high_res_1.clone(),
                points,
                labels,
            );
            (
                masks.into_data().try_to_vec::<f32>(),
                iou.into_data().try_to_vec::<f32>(),
            )
        })
        .ok_or_else(|| failed("could not read the clicks"))?;
        match (masks, scores) {
            (Ok(masks), Ok(scores)) => choose_proposal(&masks, &scores),
            _ => Err(failed("could not read the clicks")),
        }
    }
}

#[cfg(test)]
mod tests;
