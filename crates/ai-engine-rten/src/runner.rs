//! SAM 2.1 on rten as a [`ModelRunner`]: the whole of what an app gets.
//!
//! The request and answer are the shared `image.segment` contract (`craft_ai_core::segment`): a
//! PNG or JPEG `source` (told apart by its first bytes), and clicks (at least one on the object)
//! or a box in params. The answer is a grayscale PNG mask the size of the picture as a person
//! sees it, white where selected, then the [`Found`] result saying whether anything matched. A
//! JPEG's EXIF turn is applied, so the mask is upright too.
//!
//! This model selects by clicking. It cannot read a description, so a request with words is
//! refused in plain terms rather than answered with a guess.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use craft_ai_core::segment::{self, Found};
use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, ModelId, ModelInfo, Output,
    Payload, Request,
};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};

use crate::picture::{decode_picture, encode_mask, quantize};
use crate::resample::{coverage, preprocess};
use crate::sam21::{Area, Backend, Click, DECODER_FILE, ENCODER_FILE, Rten};

/// The engine and model ids as the catalogue writes them (`NativeRoute`).
pub const ENGINE: &str = "rten";
pub const MODEL: &str = "sam21";

/// Every runner this crate offers, for an `EngineRegistry`.
pub fn runners() -> Vec<Arc<dyn ModelRunner>> {
    vec![Arc::new(Sam21Runner)]
}

/// SAM 2.1 on the rten engine. It runs on the processor, on every system.
pub struct Sam21Runner;

impl ModelRunner for Sam21Runner {
    /// The engine name the catalogue writes.
    fn engine(&self) -> &str {
        ENGINE
    }

    /// Describes the model: `image.segment` with a required picture and an accepted prompt.
    fn model(&self) -> ModelInfo {
        let spec = |capability: CapabilityId| {
            // The tool offers a prompt input too; this model accepts one so it can be chosen for
            // the tool, and says plainly at run time that it reads clicks, not words.
            CapabilitySpec::new(capability)
                .accepting(InputRole::Source, 1)
                .accepting(InputRole::Prompt, 1)
                .requiring(InputRole::Source)
        };
        let mut info = ModelInfo::new(model_id(), "SAM 2.1, built in");
        if let Ok(capability) = CapabilityId::parse("image.segment") {
            info = info.with(spec(capability));
        }
        info
    }

    /// Loads both ONNX files when `weights` is the encoder and the decoder is beside it, and says
    /// what is missing otherwise.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        let dir = weights.parent().filter(|_| {
            weights
                .file_name()
                .is_some_and(|n| n == std::ffi::OsStr::new(ENCODER_FILE))
        });
        let Some(dir) = dir.filter(|d| d.join(DECODER_FILE).is_file() && weights.is_file()) else {
            return Err(AiError::unsupported(
                "SAM 2.1 needs encoder.onnx and decoder.onnx together in one folder. Open Set up AI to download them.",
            ));
        };
        Ok(Box::new(Loaded::new(Rten::load(dir)?)))
    }
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `sam21` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("sam21 is a valid model id")
}

/// The model in memory, and the last picture it looked at.
pub struct Loaded<B: Backend> {
    backend: B,
    last: Option<(u64, B::Features)>,
}

impl<B: Backend> Loaded<B> {
    /// Wraps a backend with no picture looked at yet.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            last: None,
        }
    }
}

impl<B: Backend + 'static> LoadedModel for Loaded<B> {
    /// Reads the clicks and box, looks at the picture once (kept for the next request on the same
    /// picture), asks the decoder, and returns the mask at the picture's size and the [`Found`] result.
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        let (clicks, area) = read_prompt(request)?;
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
                "SAM 2.1 needs the picture itself, not a link to it.",
            ));
        };
        let picture = decode_picture(data)?;
        ctx.progress(5);
        ctx.check_cancelled()?;

        let key = {
            let mut h = DefaultHasher::new();
            data.hash(&mut h);
            h.finish()
        };
        if self.last.as_ref().is_none_or(|(k, _)| *k != key) {
            self.last = None;
            ctx.notice(
                "Looking at the picture. This is the slow part and happens once per picture.",
            );
            let prepared = preprocess(&picture.bytes, picture.width, picture.height)?;
            ctx.check_cancelled()?;
            self.last = Some((key, self.backend.encode(prepared)?));
        }
        ctx.progress(80);
        ctx.check_cancelled()?;

        let Some((_, features)) = self.last.as_ref() else {
            return Err(AiError::new(
                ErrorKind::Internal,
                "The picture was not kept.",
            ));
        };
        let prediction = self.backend.segment(features, &clicks, area)?;
        ctx.progress(95);

        // A mask with no pixel above zero is the model saying there is nothing there.
        let found = prediction.logits.iter().any(|l| *l > 0.0);
        let mut alpha = if found {
            coverage(&prediction.logits, picture.width, picture.height)?
        } else {
            vec![0.0; picture.width * picture.height]
        };
        // What counts is the mask the person is handed: a positive speck that the resampling to a
        // small picture never samples leaves no pixel selected, so it is no match.
        let found = found && alpha.iter().any(|a| quantize(*a) > 127);
        if !found {
            ctx.notice("Nothing was selected at that spot.");
            alpha.fill(0.0);
        }
        let png = encode_mask(&alpha, picture.width, picture.height)?;
        let result = Found {
            matched: found,
            score: found.then_some(prediction.score.clamp(0.0, 1.0)),
            name: None,
        };
        Ok(vec![
            Output {
                payload: Payload::inline("image/png", png)?,
                label: None,
            },
            segment::found_output(&result)?,
        ])
    }
}

/// Reads the clicks and the box, and refuses a request that asks for words or gives neither a
/// click on the object nor a box.
pub fn read_prompt(request: &Request) -> Result<(Vec<Click>, Option<Area>), AiError> {
    if segment::description(request).is_some() {
        return Err(AiError::unsupported(
            "This option selects by clicking on the picture. To select by describing it, choose another in Set up AI.",
        ));
    }
    if request
        .inputs
        .iter()
        .any(|i| i.role == InputRole::Prompt && !matches!(i.payload, Payload::Text { .. }))
    {
        return Err(AiError::unsupported(
            "This option reads a prompt only as text, and it selects by clicking on the picture.",
        ));
    }
    let clicks = segment::clicks(request)?;
    let area = segment::area(request)?;
    if area.is_none() && !clicks.iter().any(|c| c.positive) {
        return Err(AiError::invalid(
            "Click on what you want to select, or draw a box around it.",
        ));
    }
    Ok((clicks, area))
}

#[cfg(test)]
mod tests;
