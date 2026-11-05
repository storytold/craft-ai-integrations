//! SAM 2.1 as two ONNX graphs: an image encoder that runs once per picture and a small prompt
//! decoder that runs once per click. [`Backend`] is the seam between the engine contract and the
//! runtime, so the runner can be tested without the real files; [`Rten`] is the real one.
//!
//! The files are the published export (`encoder.onnx` and `decoder.onnx`, Apache-2.0, opset 17).
//! Their interface, as the export documents it:
//!
//! - encoder: `image` `[1, 3, 1024, 1024]` to `image_embed` `[1, 256, 64, 64]`, `high_res_0`
//!   `[1, 32, 256, 256]` and `high_res_1` `[1, 64, 128, 128]`;
//! - decoder: those three, `point_coords` `[1, N, 2]` (pixels of the 1024 square) and
//!   `point_labels` `[1, N]` (1 for a click on the object, 0 for a click off it, 2 and 3 for the
//!   top-left and bottom-right corners of a box) to `mask_logits` `[1, 4, 256, 256]` and `iou`
//!   `[1, 4]`.

use std::path::Path;

use craft_ai_core::{AiError, ErrorKind};
use rten::{Model, NodeId, Value};
use rten_tensor::{AsView, Tensor};

use crate::resample::{MASK_SIDE, SIDE};

/// The two files, side by side in one folder. The first is the weights file the connection records.
pub const ENCODER_FILE: &str = "encoder.onnx";
pub const DECODER_FILE: &str = "decoder.onnx";

/// The decoder's mask proposals: one from a single prediction, three from the multi-mask head.
const PROPOSALS: usize = 4;

pub use craft_ai_core::segment::{Area, Click};

/// The decoder's point labels for the two corners of a box.
const BOX_TOP_LEFT: i32 = 2;
const BOX_BOTTOM_RIGHT: i32 = 3;

/// The mask the model is most sure of, as logits over the 256 square, and how sure it is.
#[derive(Clone, Debug)]
pub struct Prediction {
    pub logits: Vec<f32>,
    pub score: f32,
}

/// What the runner needs from a model: look at a picture once, then answer clicks on it.
pub trait Backend: Send {
    /// What looking at a picture leaves behind, kept so a second click on it is quick.
    type Features: Send;

    /// Looks at a prepared picture (`[3, 1024, 1024]` floats, see `resample::preprocess`).
    fn encode(&self, picture: Vec<f32>) -> Result<Self::Features, AiError>;

    /// Answers clicks, and a box if there is one, on the picture those features came from.
    fn segment(
        &self,
        features: &Self::Features,
        clicks: &[Click],
        area: Option<Area>,
    ) -> Result<Prediction, AiError>;
}

/// A provider error that says what SAM 2.1 was doing when the runtime failed.
fn failed(what: &str, e: impl std::fmt::Display) -> AiError {
    AiError::new(ErrorKind::Provider, format!("SAM 2.1 {what}: {e}"))
}

/// Said when the files load but are not the export this runner knows.
fn not_this_model() -> AiError {
    AiError::unsupported(
        "These files are not the SAM 2.1 model Craft AI expects. Open Set up AI to download them again.",
    )
}

/// What the encoder leaves behind: the three tensors the decoder reads.
pub(crate) struct Features {
    embed: Value,
    high_res_0: Value,
    high_res_1: Value,
}

/// The real backend: both graphs loaded by rten, with the node ids looked up once.
pub(crate) struct Rten {
    encoder: Model,
    decoder: Model,
    image: NodeId,
    encoded: [NodeId; 3],
    decoder_in: [NodeId; 5],
    decoder_out: [NodeId; 2],
}

impl Rten {
    /// Loads `encoder.onnx` and `decoder.onnx` from `dir` and checks they have the interface above.
    pub(crate) fn load(dir: &Path) -> Result<Self, AiError> {
        let read = |file: &str| {
            Model::load_file(dir.join(file)).map_err(|e| failed("could not be loaded", e))
        };
        let encoder = read(ENCODER_FILE)?;
        let decoder = read(DECODER_FILE)?;
        let id = |model: &Model, name: &str| model.node_id(name).map_err(|_| not_this_model());
        let image = id(&encoder, "image")?;
        let encoded = [
            id(&encoder, "image_embed")?,
            id(&encoder, "high_res_0")?,
            id(&encoder, "high_res_1")?,
        ];
        let decoder_in = [
            id(&decoder, "image_embed")?,
            id(&decoder, "high_res_0")?,
            id(&decoder, "high_res_1")?,
            id(&decoder, "point_coords")?,
            id(&decoder, "point_labels")?,
        ];
        let decoder_out = [id(&decoder, "mask_logits")?, id(&decoder, "iou")?];
        Ok(Self {
            encoder,
            decoder,
            image,
            encoded,
            decoder_in,
            decoder_out,
        })
    }
}

/// The decoder's prompt points: each click, then the two corners of a box if there is one.
/// Coordinates are in pixels of the 1024 square, with no half-pixel shift, as the export expects.
/// Labels are 1 for a click on the object, 0 for a click off it, 2 and 3 for the box corners.
pub fn prompt_points(clicks: &[Click], area: Option<Area>) -> (Vec<f32>, Vec<i32>) {
    let mut coords = Vec::with_capacity((clicks.len() + 2) * 2);
    let mut labels = Vec::with_capacity(clicks.len() + 2);
    let mut push = |x: f32, y: f32, label: i32| {
        coords.push(x * SIDE as f32);
        coords.push(y * SIDE as f32);
        labels.push(label);
    };
    for click in clicks {
        push(click.x, click.y, i32::from(click.positive));
    }
    if let Some(b) = area {
        push(b.x0, b.y0, BOX_TOP_LEFT);
        push(b.x1, b.y1, BOX_BOTTOM_RIGHT);
    }
    (coords, labels)
}

/// The index of the proposal the model rates highest, ignoring scores that are not numbers; the
/// first when none is usable.
pub fn best_proposal(scores: &[f32]) -> usize {
    scores
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_finite())
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}

/// The floats of an output, when it has the length it should.
fn floats(value: Value, len: usize) -> Result<Vec<f32>, AiError> {
    let tensor = value.into_tensor::<f32>().ok_or_else(not_this_model)?;
    let data = tensor.to_vec();
    if data.len() == len {
        Ok(data)
    } else {
        Err(not_this_model())
    }
}

impl Backend for Rten {
    type Features = Features;

    /// Runs the encoder on the prepared picture and keeps the three tensors the decoder reads.
    fn encode(&self, picture: Vec<f32>) -> Result<Features, AiError> {
        let input = Tensor::from_data(&[1, 3, SIDE, SIDE], picture);
        let out = self
            .encoder
            .run(
                vec![(self.image, Value::from(input).into())],
                &self.encoded,
                None,
            )
            .map_err(|e| failed("could not look at the picture", e))?;
        let mut out = out.into_iter();
        match (out.next(), out.next(), out.next()) {
            (Some(embed), Some(high_res_0), Some(high_res_1)) => Ok(Features {
                embed,
                high_res_0,
                high_res_1,
            }),
            _ => Err(not_this_model()),
        }
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
        let [embed, high_res_0, high_res_1, coords_id, labels_id] = self.decoder_in;
        let inputs = vec![
            (embed, features.embed.clone().into()),
            (high_res_0, features.high_res_0.clone().into()),
            (high_res_1, features.high_res_1.clone().into()),
            (
                coords_id,
                Value::from(Tensor::from_data(&[1, n, 2], coords)).into(),
            ),
            (
                labels_id,
                Value::from(Tensor::from_data(&[1, n], labels)).into(),
            ),
        ];
        let out = self
            .decoder
            .run(inputs, &self.decoder_out, None)
            .map_err(|e| failed("could not read the clicks", e))?;
        let mut out = out.into_iter();
        let (Some(masks), Some(scores)) = (out.next(), out.next()) else {
            return Err(not_this_model());
        };
        let side = MASK_SIDE * MASK_SIDE;
        let masks = floats(masks, PROPOSALS * side)?;
        let scores = floats(scores, PROPOSALS)?;
        let best = best_proposal(&scores);
        let logits = masks
            .get(best * side..(best + 1) * side)
            .ok_or_else(not_this_model)?
            .to_vec();
        Ok(Prediction {
            logits,
            score: scores.get(best).copied().unwrap_or(0.0),
        })
    }
}

#[cfg(test)]
mod tests;
