//! FLUX.1 Fill [pro] (`POST /flux-pro-1.0-fill`): the request body, the mask and the result address.
//!
//! BFL reads a separate black-and-white mask, white where it should paint and black where the
//! picture stays. The craft mask is a PNG with the same meaning (as Select subject produces and
//! PhotoCraft sends). A mask that carries transparency is read the other common way instead,
//! transparent where to paint, as OpenAI's edits take it. Either way BFL is sent a binary grayscale
//! PNG, white where to paint, so a stray grey or an alpha channel cannot be read differently there.
//!
//! BFL documents no size limit for this endpoint. The bounds here are ours: 8,192 pixels a side and
//! 32 megapixels, checked on the mask, and on the source when it is a PNG.
use craft_ai_core::payload::base64_encode;
use craft_ai_core::{AiError, InputRole, Payload, Request, endpoint::Endpoint};
use craft_ai_media::mask::{self, Bounds};
use serde_json::{Map, Value, json};

pub(crate) const MODEL: &str = "flux-pro-1.0-fill";
pub(crate) const PATH: &str = "/flux-pro-1.0-fill";
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const BOUNDS: Bounds = Bounds {
    service: "FLUX Fill",
    max_side: MAX_SIDE,
    max_pixels: MAX_PIXELS,
};

/// What a picture is, from its first bytes.
fn sniff(data: &[u8]) -> Option<&'static str> {
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

/// The media type of a returned picture, or `None` when it is not PNG, JPEG or WebP.
pub(crate) fn result_media(data: &[u8]) -> Option<&'static str> {
    sniff(data)
}

/// Reads the craft mask and returns BFL's: an opaque 8-bit grayscale PNG, white where to paint,
/// black where to keep (see [`craft_ai_media::mask::binary`]).
pub(crate) fn bfl_mask(data: &[u8]) -> Result<(Vec<u8>, u32, u32), AiError> {
    mask::binary(data, BOUNDS)
}

/// Copies the options BFL documents for this endpoint, each checked against its documented range.
/// Anything else is refused, so a misspelt option is never silently dropped.
fn options(request: &Request, body: &mut Map<String, Value>) -> Result<(), AiError> {
    let bad = |name: &str, rule: &str| AiError::invalid(format!("FLUX Fill {name} must be {rule}"));
    for (name, value) in &request.params {
        let ok = match name.as_str() {
            "steps" => value.as_u64().is_some_and(|n| (15..=50).contains(&n)),
            "guidance" => value.as_f64().is_some_and(|g| (1.5..=100.0).contains(&g)),
            "safety_tolerance" => value.as_u64().is_some_and(|n| n <= 6),
            "seed" => value.is_i64() || value.is_u64(),
            "prompt_upsampling" => value.is_boolean(),
            "output_format" => matches!(value.as_str(), Some("png" | "jpeg" | "webp")),
            _ => {
                return Err(AiError::unsupported(format!(
                    "FLUX Fill has no option called {name}"
                )));
            }
        };
        if !ok {
            return Err(bad(
                name,
                match name.as_str() {
                    "steps" => "a whole number from 15 to 50",
                    "guidance" => "a number from 1.5 to 100",
                    "safety_tolerance" => "a whole number from 0 to 6",
                    "seed" => "a whole number",
                    "prompt_upsampling" => "true or false",
                    _ => "png, jpeg or webp",
                },
            ));
        }
        body.insert(name.clone(), value.clone());
    }
    Ok(())
}

/// The JSON body for one fill. Everything is checked here, before anything is sent.
pub(crate) fn body(request: &Request) -> Result<Value, AiError> {
    let prompt = request
        .inputs
        .iter()
        .find(|i| i.role == InputRole::Prompt)
        .and_then(|i| match &i.payload {
            Payload::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
        .ok_or_else(|| AiError::invalid("FLUX Fill needs a non-empty text prompt"))?;
    let source = mask::inline(request, BOUNDS, InputRole::Source, "source image")?;
    sniff(source)
        .ok_or_else(|| AiError::invalid("the source image is not a PNG, JPEG or WebP image"))?;
    let (mask, width, height) = bfl_mask(mask::inline(request, BOUNDS, InputRole::Mask, "mask")?)?;
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
    let mut body = Map::new();
    body.insert("prompt".into(), json!(prompt));
    body.insert("image".into(), json!(base64_encode(source)));
    body.insert("mask".into(), json!(base64_encode(&mask)));
    // Lossless by default: the fill is composited back into a document.
    body.insert("output_format".into(), json!("png"));
    options(request, &mut body)?;
    Ok(Value::Object(body))
}

/// Whether a result address may be downloaded: https on a BFL host (`delivery.*.bfl.ai` today;
/// BFL asks callers not to depend on the exact name), or, only when the API itself is on this
/// computer (tests), a loopback address.
pub(crate) fn delivery_allowed(uri: &str, api_is_loopback: bool) -> bool {
    let rest = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"));
    let Some(rest) = rest else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let scheme = if uri.starts_with("https://") {
        "https://"
    } else {
        "http://"
    };
    let Ok(endpoint) = Endpoint::parse(&format!("{scheme}{authority}")) else {
        return false;
    };
    if uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if endpoint.is_loopback() {
        return api_is_loopback;
    }
    endpoint.https && endpoint.port.is_none_or(|p| p == 443) && endpoint.host.ends_with(".bfl.ai")
}

#[cfg(test)]
mod tests;
