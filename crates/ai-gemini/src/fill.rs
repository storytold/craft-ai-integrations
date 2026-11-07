//! Generative fill on Gemini's image models: the request body, the mask, and reading the answer.
//!
//! Gemini edits from instructions and pictures; it takes no mask parameter. The craft mask (white,
//! or for a mask with transparency transparent, where to paint) is turned into an opaque black and
//! white PNG, white where to paint, and sent as a second picture after the source, with an
//! instruction that says what it means. Nothing stops the model from changing pixels outside the
//! mask, so the caller should keep only the masked part of the result (PhotoCraft does).
//!
//! The instruction allows for the area under the mask already having been blurred or dimmed out
//! (PhotoCraft does that so the model cannot paint back what was there): whatever the area shows
//! is treated as a placeholder to replace, not as content to keep.
use craft_ai_core::payload::{base64_decode, base64_encode};
use craft_ai_core::{AiError, ErrorKind, InputRole, Payload, Request};
use craft_ai_media::mask::{self, Bounds, ReferenceRule};
use serde_json::{Value, json};

/// Image models offered, best default first: (id, label).
pub(crate) const MODELS: &[(&str, &str)] = &[
    ("gemini-nano-banana-2.1", "Nano Banana 2.1"),
    ("gemini-3-pro-image", "Nano Banana Pro"),
    ("gemini-3.1-flash-image", "Nano Banana 2"),
    (
        "gemini-3.1-flash-lite-image",
        "Nano Banana 2 Lite (fastest, 1K only)",
    ),
];

/// Output aspect ratios the image models accept.
const RATIOS: &[(u32, u32)] = &[
    (1, 1),
    (3, 2),
    (2, 3),
    (3, 4),
    (4, 3),
    (4, 5),
    (5, 4),
    (9, 16),
    (16, 9),
    (21, 9),
];

/// Our own bound on the pictures sent (Google documents a request size limit, not a pixel one).
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const BOUNDS: Bounds = Bounds {
    service: "Gemini fill",
    max_side: MAX_SIDE,
    max_pixels: MAX_PIXELS,
};

/// Reference pictures one fill may carry after the source and the mask. The count and size are our
/// own bounds, kept low because the service's picture limits differ by model and the source and
/// mask already take two places. Read by the client's route check through
/// `GeminiProvider::MAX_REFERENCES`, so the two cannot disagree.
pub(crate) const REFERENCES: ReferenceRule = ReferenceRule {
    max_count: 3,
    max_bytes: 10 * 1024 * 1024,
    png_only: false,
};

/// The most one fill request body may be, as the bytes sent. Google documents a 20 MB total
/// request size for inline data (text and every file) on the Gemini API; its Interactions pages
/// state no figure, so the documented one is used here, in decimal megabytes to stay on the safe
/// side. Three references within [`REFERENCES`] can reach 30 MB before encoding, so this is what
/// stops a request the service would refuse.
pub(crate) const MAX_REQUEST_BYTES: usize = 20_000_000;

/// The request body for one fill as the JSON bytes to send, refused when longer than
/// [`MAX_REQUEST_BYTES`]. The serialised bytes are measured, so base64 padding, the escaping of
/// the prompt and every field around the pictures all count. Nothing has been sent when it refuses.
pub(crate) fn encoded_body(model: &str, request: &Request) -> Result<Vec<u8>, AiError> {
    let bytes = serde_json::to_vec(&body(model, request)?)
        .map_err(|_| AiError::new(ErrorKind::Internal, "could not encode Gemini request"))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(AiError::invalid(
            "the source, mask, reference pictures and prompt add up to more than Gemini's 20 MB \
             request limit; send fewer or smaller reference pictures, or a shorter prompt",
        ));
    }
    Ok(bytes)
}

/// Whether `model` may be asked for: one of [`MODELS`], or any well-formed `gemini-…-image…` id so
/// a newer model works without an update to this crate.
pub(crate) fn model_allowed(model: &str) -> bool {
    MODELS.iter().any(|(id, _)| *id == model)
        || (model.len() <= 80
            && model.starts_with("gemini-")
            && model.contains("image")
            && model
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'))
}

/// What a picture is, from its first bytes.
pub(crate) fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.get(0..4) == Some(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// The supported ratio closest to `w`:`h` (compared on a log scale, so 2:1 and 1:2 are as far from
/// 1:1 as each other).
pub(crate) fn nearest_ratio(w: u32, h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let want = (f64::from(w) / f64::from(h)).ln();
    RATIOS
        .iter()
        .copied()
        .min_by(|a, b| {
            let d = |r: (u32, u32)| ((f64::from(r.0) / f64::from(r.1)).ln() - want).abs();
            d(*a).total_cmp(&d(*b))
        })
        .unwrap_or((1, 1))
}

/// The output size token: asked for in `image_size`, else enough for the picture sent (its long
/// side up to 1,024 px is 1K, up to 2,048 px 2K, larger 4K). Lite models only make 1K; [`body`]
/// refuses a larger size asked of them before this is reached.
fn size_token(model: &str, asked: Option<&str>, w: u32, h: u32) -> &'static str {
    if model.contains("lite") {
        return "1K";
    }
    match asked {
        Some("1K") => "1K",
        Some("2K") => "2K",
        Some("4K") => "4K",
        _ => match w.max(h) {
            0..=1024 => "1K",
            1025..=2048 => "2K",
            _ => "4K",
        },
    }
}

/// Reads the craft mask and returns the one Gemini is shown: an opaque 8-bit grayscale PNG, white where to paint,
/// black where to keep (see [`craft_ai_media::mask::binary`]).
pub(crate) fn shown_mask(data: &[u8]) -> Result<(Vec<u8>, u32, u32), AiError> {
    mask::binary(data, BOUNDS)
}

/// The instruction that stands in for a mask parameter. An empty prompt means "what would be
/// there without whatever stood in it": the background and anything crossing the edge carry on
/// through it.
pub fn fill_instruction(prompt: &str) -> String {
    mask::fill_instruction(prompt)
}

/// The Interactions request body for one fill. Everything is checked here, before anything is
/// sent.
pub(crate) fn body(model: &str, request: &Request) -> Result<Value, AiError> {
    let prompt = match request.inputs.iter().find(|i| i.role == InputRole::Prompt) {
        None => "",
        Some(i) => match &i.payload {
            Payload::Text { text } => text.as_str(),
            _ => return Err(AiError::invalid("the Gemini fill prompt must be text")),
        },
    };
    let mut asked_size = None;
    for (name, value) in &request.params {
        match name.as_str() {
            "image_size" => match value.as_str() {
                Some(s @ ("1K" | "2K" | "4K")) => asked_size = Some(s),
                _ => return Err(AiError::invalid("Gemini image_size must be 1K, 2K or 4K")),
            },
            _ => {
                return Err(AiError::unsupported(format!(
                    "Gemini fill has no option called {name}"
                )));
            }
        }
    }
    // Lite models only make 1K pictures; a larger size asked for is refused, not quietly lowered.
    if model.contains("lite") && matches!(asked_size, Some("2K" | "4K")) {
        return Err(AiError::unsupported(
            "this Gemini model makes 1K pictures only; ask for 1K or leave the size out",
        ));
    }
    let source = mask::inline(request, BOUNDS, InputRole::Source, "source image")?;
    let media = sniff(source)
        .ok_or_else(|| AiError::invalid("the source image is not a PNG, JPEG or WebP image"))?;
    let (mask, width, height) =
        shown_mask(mask::inline(request, BOUNDS, InputRole::Mask, "mask")?)?;
    // Every accepted format is measured, so a JPEG or WebP source meets the same limits and
    // mask-size rule as a PNG before anything is sent.
    let (w, h) = craft_ai_media::image_size(source)
        .ok_or_else(|| AiError::invalid("the source image's size could not be read"))?;
    BOUNDS.check(w, h)?;
    if (w, h) != (width, height) {
        return Err(AiError::invalid(
            "the mask must be the same size as the source image",
        ));
    }
    // References are checked last and sent after the mask, in request order, so the instruction's
    // "third image onwards" always names them.
    let references = mask::references(request, BOUNDS, REFERENCES)?;
    // Each picture is within its own limit here; their total is checked in [`encoded_body`].
    let instruction = mask::fill_instruction_with_references(prompt, references.len());
    let mut input = vec![
        json!({"type": "text", "text": instruction}),
        json!({"type": "image", "mime_type": media, "data": base64_encode(source)}),
        json!({"type": "image", "mime_type": "image/png", "data": base64_encode(&mask)}),
    ];
    input.extend(
        references
            .iter()
            .map(|r| json!({"type": "image", "mime_type": r.media, "data": base64_encode(r.data)})),
    );
    let (a, b) = nearest_ratio(width, height);
    Ok(json!({
        "model": model,
        "input": input,
        "response_format": {
            "type": "image",
            // The Interactions API returns JPEG only ("Supported values: 'image/jpeg'").
            "mime_type": "image/jpeg",
            "aspect_ratio": format!("{a}:{b}"),
            "image_size": size_token(model, asked_size, width, height),
        },
    }))
}

/// What a successful answer held.
pub(crate) enum Answer {
    Image(&'static str, Vec<u8>),
    /// No picture: what the model said instead (a refusal or a safety block), maybe empty.
    Text(String),
}

/// Reads an Interactions answer: the first picture in `output_image`, else an image block in
/// `steps[].content[]` or `outputs[]`; failing that, the model's text.
pub(crate) fn read_answer(v: &Value) -> Result<Answer, AiError> {
    let mut candidates: Vec<&Value> = Vec::new();
    candidates.extend(v.get("output_image"));
    for key in ["steps", "outputs"] {
        for item in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            candidates.push(item);
            candidates.extend(
                item.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            );
        }
    }
    for c in candidates {
        let is_image = c
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|t| t == "image");
        let Some(data) = c.get("data").and_then(Value::as_str).filter(|_| is_image) else {
            continue;
        };
        let bytes = base64_decode(data)
            .ok_or_else(|| AiError::new(ErrorKind::Provider, "Gemini image data is invalid"))?;
        let media = craft_ai_media::decoded_result(&bytes).ok_or_else(|| {
            AiError::new(
                ErrorKind::Provider,
                "Gemini returned a picture that is not a readable PNG, JPEG or WebP",
            )
        })?;
        return Ok(Answer::Image(media, bytes));
    }
    Ok(Answer::Text(output_text(v)))
}

fn output_text(v: &Value) -> String {
    if let Some(t) = v.get("output_text").and_then(Value::as_str) {
        return t.trim().to_owned();
    }
    let mut out = Vec::new();
    for key in ["steps", "outputs"] {
        for item in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            let blocks = item.get("content").and_then(Value::as_array);
            for c in blocks.into_iter().flatten().chain(std::iter::once(item)) {
                if c.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(t) = c.get("text").and_then(Value::as_str)
                {
                    out.push(t.trim());
                }
            }
        }
    }
    out.join(" ")
}

/// `s` cut to at most `max` characters, with an ellipsis when cut.
pub(crate) fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", s.get(..i).unwrap_or_default()),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod size_tests;
