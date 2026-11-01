//! DeepFilterNet 3 on rten: the built-in runner behind `audio.enhance`. It takes the voice out of
//! background noise and room sound, on the processor, with nothing leaving the computer.
//!
//! **Request:** the recording as the `source` input, an inline WAV (`audio/wav`) at 48 kHz in
//! 16, 24 or 32-bit PCM or 32-bit float, up to [`wav::MAX_CHANNELS`] channels. One optional
//! param, `max_reduction_db` (0 to 100): the most the noise is turned down, so some room sound
//! can stay. Without it the reduction is as strong as the model makes it.
//!
//! **Answer:** one `audio/wav` output, 32-bit float, with the same rate, channels and length as
//! the input and lined up with it sample for sample, so an app can put it in place of the
//! original. Each channel is processed on its own.
//!
//! **Files:** the publisher's ONNX export of DeepFilterNet 3 (Rikorose/DeepFilterNet, MIT OR
//! Apache-2.0): `enc.onnx` (the weights file the connection records), `erb_dec.onnx` and
//! `df_dec.onnx`, side by side. The catalogue names their sizes and SHA-256. Their interface:
//!
//! - enc: `feat_erb` `[1, 1, T, 32]`, `feat_spec` `[1, 2, T, 96]` to `e0`–`e3`, `emb`, `c0`, `lsnr`;
//! - erb_dec: `emb`, `e3`, `e2`, `e1`, `e0` to `m` `[1, 1, T, 32]`, the ERB gains;
//! - df_dec: `emb`, `c0` to `coefs` `[1, T, 96, 10]`, five complex taps per bin.

use std::path::Path;
use std::sync::Arc;

use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, ModelId, ModelInfo, Output,
    Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use rten::{Model, NodeId, Value};
use rten_tensor::Tensor;
use rten_tensor::prelude::*;

pub(crate) mod dsp;
pub(crate) mod wav;

use dsp::{Analysed, DF_ORDER, Estimate, NB_DF, NB_ERB, SAMPLE_RATE};

/// The model id as the catalogue writes it (`NativeRoute`).
pub const DFN3_MODEL: &str = "deepfilternet3";
/// The three files, side by side in one folder. The first is the weights file the connection
/// records.
pub const DFN3_ENCODER_FILE: &str = "enc.onnx";
pub const DFN3_ERB_DECODER_FILE: &str = "erb_dec.onnx";
pub const DFN3_DF_DECODER_FILE: &str = "df_dec.onnx";
/// The capability this runner answers.
const CAPABILITY: &str = "audio.enhance";
/// The media type of the answer.
pub const WAV: &str = "audio/wav";
/// The param that caps the reduction, in dB.
pub const MAX_REDUCTION_DB: &str = "max_reduction_db";

/// What the runner needs from a model: gains and filter taps for one analysed channel. The seam
/// lets the runner be tested without the real files.
pub(crate) trait Estimator: Send {
    fn estimate(&self, analysed: &Analysed) -> Result<Estimate, AiError>;
}

/// DeepFilterNet 3 on the rten engine. It runs on the processor, on every system.
pub struct Dfn3Runner;

impl ModelRunner for Dfn3Runner {
    fn engine(&self) -> &str {
        crate::ENGINE
    }

    /// `audio.enhance` with a required recording.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "DeepFilterNet 3, built in");
        if let Ok(capability) = CapabilityId::parse(CAPABILITY) {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .requiring(InputRole::Source),
            );
        }
        info
    }

    /// Loads the three ONNX files when `weights` is `enc.onnx` and the other two are beside it,
    /// and says what is missing otherwise.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        let named = weights
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new(DFN3_ENCODER_FILE));
        let dir = weights.parent().filter(|d| {
            named
                && weights.is_file()
                && d.join(DFN3_ERB_DECODER_FILE).is_file()
                && d.join(DFN3_DF_DECODER_FILE).is_file()
        });
        let Some(dir) = dir else {
            return Err(AiError::unsupported(
                "DeepFilterNet 3 needs enc.onnx, erb_dec.onnx and df_dec.onnx together in one folder. Open Set up AI to download them.",
            ));
        };
        Ok(Box::new(Enhancer::new(Rten::load(dir)?)))
    }
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `deepfilternet3` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(DFN3_MODEL).expect("deepfilternet3 is a valid model id")
}

/// The model in memory, behind the request handling.
pub(crate) struct Enhancer<E: Estimator> {
    estimator: E,
    widths: Vec<usize>,
}

impl<E: Estimator> Enhancer<E> {
    pub(crate) fn new(estimator: E) -> Self {
        Self {
            estimator,
            widths: dsp::erb_widths(),
        }
    }
}

impl<E: Estimator + 'static> LoadedModel for Enhancer<E> {
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        let keep = read_keep(request)?;
        let audio = wav::decode(read_source(request)?)?;
        if audio.sample_rate != SAMPLE_RATE {
            return Err(AiError::invalid(format!(
                "DeepFilterNet 3 works on 48 kHz audio, and this recording is {} Hz. Convert it to 48 kHz first.",
                audio.sample_rate
            )));
        }
        let total = audio.channels.len();
        let mut out = Vec::with_capacity(total);
        for (i, channel) in audio.channels.iter().enumerate() {
            ctx.check_cancelled()?;
            out.push(dsp::process(channel, &self.widths, keep, |a| {
                self.estimator.estimate(a)
            })?);
            ctx.progress(u8::try_from((i + 1) * 100 / total).unwrap_or(99));
        }
        ctx.check_cancelled()?;
        let data = wav::encode(audio.sample_rate, &out)?;
        Ok(vec![Output {
            payload: Payload::inline(WAV, data)?,
            label: None,
        }])
    }
}

/// The recording: an inline WAV, not a link.
fn read_source(request: &Request) -> Result<&[u8], AiError> {
    let source = request
        .inputs
        .iter()
        .find(|i| i.role == InputRole::Source)
        .map(|i| &i.payload);
    match source {
        Some(Payload::Inline { media_type, data }) => {
            if matches!(
                media_type.as_str(),
                "audio/wav" | "audio/x-wav" | "audio/wave"
            ) {
                Ok(data)
            } else {
                Err(AiError::invalid("The audio must be a WAV file."))
            }
        }
        Some(Payload::Handle(_)) => Err(AiError::unsupported(
            "DeepFilterNet 3 needs the audio itself, not a link to it.",
        )),
        _ => Err(AiError::invalid("A recording is needed.")),
    }
}

/// How much of the noisy signal to mix back in, from `max_reduction_db`. None asked: nothing.
fn read_keep(request: &Request) -> Result<f32, AiError> {
    let Some(value) = request.params.get(MAX_REDUCTION_DB) else {
        return Ok(0.);
    };
    match value.as_f64() {
        Some(db) if db.is_finite() && (0. ..=100.).contains(&db) => {
            Ok(10f64.powf(-db / 20.) as f32)
        }
        _ => Err(AiError::invalid(
            "max_reduction_db must be a number of decibels from 0 to 100.",
        )),
    }
}

/// A provider error that says what DeepFilterNet 3 was doing when the runtime failed.
fn failed(what: &str, e: impl std::fmt::Display) -> AiError {
    AiError::new(ErrorKind::Provider, format!("DeepFilterNet 3 {what}: {e}"))
}

/// Said when the files load but are not the export this runner knows.
fn not_this_model() -> AiError {
    AiError::unsupported(
        "These files are not the DeepFilterNet 3 model Craft AI expects. Open Set up AI to download them again.",
    )
}

/// The real estimator: the three graphs loaded by rten, with the node ids looked up once.
pub(crate) struct Rten {
    enc: Model,
    erb_dec: Model,
    df_dec: Model,
    enc_in: [NodeId; 2],
    /// `e0`, `e1`, `e2`, `e3`, `emb`, `c0`.
    enc_out: [NodeId; 6],
    /// `emb`, `e3`, `e2`, `e1`, `e0`, then `m`.
    erb_io: [NodeId; 6],
    /// `emb`, `c0`, then `coefs`.
    df_io: [NodeId; 3],
}

impl Rten {
    fn load(dir: &Path) -> Result<Self, AiError> {
        let read = |file: &str| {
            Model::load_file(dir.join(file)).map_err(|e| failed("could not be loaded", e))
        };
        let (enc, erb_dec, df_dec) = (
            read(DFN3_ENCODER_FILE)?,
            read(DFN3_ERB_DECODER_FILE)?,
            read(DFN3_DF_DECODER_FILE)?,
        );
        let id = |m: &Model, name: &str| m.node_id(name).map_err(|_| not_this_model());
        Ok(Self {
            enc_in: [id(&enc, "feat_erb")?, id(&enc, "feat_spec")?],
            enc_out: [
                id(&enc, "e0")?,
                id(&enc, "e1")?,
                id(&enc, "e2")?,
                id(&enc, "e3")?,
                id(&enc, "emb")?,
                id(&enc, "c0")?,
            ],
            erb_io: [
                id(&erb_dec, "emb")?,
                id(&erb_dec, "e3")?,
                id(&erb_dec, "e2")?,
                id(&erb_dec, "e1")?,
                id(&erb_dec, "e0")?,
                id(&erb_dec, "m")?,
            ],
            df_io: [
                id(&df_dec, "emb")?,
                id(&df_dec, "c0")?,
                id(&df_dec, "coefs")?,
            ],
            enc,
            erb_dec,
            df_dec,
        })
    }
}

/// Runs one graph and returns its outputs in the order asked.
fn run(
    model: &Model,
    inputs: Vec<(NodeId, Value)>,
    outputs: &[NodeId],
) -> Result<Vec<Value>, AiError> {
    let inputs = inputs.into_iter().map(|(id, v)| (id, v.into())).collect();
    model
        .run(inputs, outputs, None)
        .map_err(|e| failed("failed while running", e))
}

/// The floats of a graph output, checked to be `len` long.
fn floats(value: Value, len: usize) -> Result<Vec<f32>, AiError> {
    let tensor: Tensor<f32> = value.try_into().map_err(|_| not_this_model())?;
    let data = tensor.to_vec();
    if data.len() == len {
        Ok(data)
    } else {
        Err(not_this_model())
    }
}

impl Estimator for Rten {
    fn estimate(&self, a: &Analysed) -> Result<Estimate, AiError> {
        let frames = a.spec.len();
        let erb = Tensor::from_data(&[1, 1, frames, NB_ERB], a.erb.clone());
        let cplx = Tensor::from_data(&[1, 2, frames, NB_DF], a.cplx.clone());
        let enc = run(
            &self.enc,
            vec![(self.enc_in[0], erb.into()), (self.enc_in[1], cplx.into())],
            &self.enc_out,
        )?;
        let [e0, e1, e2, e3, emb, c0] =
            <[Value; 6]>::try_from(enc).map_err(|_| not_this_model())?;
        let [ei_emb, ei_e3, ei_e2, ei_e1, ei_e0, m] = self.erb_io;
        let gains = run(
            &self.erb_dec,
            vec![
                (ei_emb, emb.clone()),
                (ei_e3, e3),
                (ei_e2, e2),
                (ei_e1, e1),
                (ei_e0, e0),
            ],
            &[m],
        )?;
        let [di_emb, di_c0, coefs] = self.df_io;
        let taps = run(&self.df_dec, vec![(di_emb, emb), (di_c0, c0)], &[coefs])?;
        let one = |v: Vec<Value>| v.into_iter().next().ok_or_else(not_this_model);
        Ok(Estimate {
            gains: floats(one(gains)?, frames * NB_ERB)?,
            coefs: floats(one(taps)?, frames * NB_DF * DF_ORDER * 2)?,
        })
    }
}

/// The runners for an `EngineRegistry`.
pub(crate) fn runner() -> Arc<dyn ModelRunner> {
    Arc::new(Dfn3Runner)
}

#[cfg(test)]
mod tests;
