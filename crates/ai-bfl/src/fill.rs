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
use craft_ai_core::{AiError, ErrorKind, InputRole, Payload, Request, endpoint::Endpoint};
use png::{BitDepth, ColorType, Transformations};
use serde_json::{Map, Value, json};
use std::io::Cursor;

pub(crate) const MODEL: &str = "flux-pro-1.0-fill";
pub(crate) const PATH: &str = "/flux-pro-1.0-fill";
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;

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

fn inline<'a>(request: &'a Request, role: InputRole, what: &str) -> Result<&'a [u8], AiError> {
    let input = request
        .inputs
        .iter()
        .find(|i| i.role == role)
        .ok_or_else(|| AiError::invalid(format!("FLUX Fill needs a {what}")))?;
    match &input.payload {
        Payload::Inline { data, .. } => Ok(data),
        _ => Err(AiError::unsupported(format!(
            "the {what} must be inline image data; this adapter does not read files or URLs"
        ))),
    }
}

fn too_big() -> AiError {
    AiError::invalid(format!(
        "pictures for FLUX Fill may be at most {MAX_SIDE} pixels a side and 32 megapixels"
    ))
}

fn check_size(width: u32, height: u32) -> Result<(), AiError> {
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(too_big());
    }
    Ok(())
}

/// A PNG's size from its header, without decoding the pixels.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(Cursor::new(data)).read_info().ok()?;
    Some((reader.info().width, reader.info().height))
}

fn bad_mask() -> AiError {
    AiError::invalid("the mask is not a readable PNG image")
}

/// Reads the craft mask and returns BFL's: an 8-bit grayscale PNG, white where to paint.
pub(crate) fn bfl_mask(data: &[u8]) -> Result<(Vec<u8>, u32, u32), AiError> {
    if sniff(data) != Some("image/png") {
        return Err(AiError::invalid("the mask must be a PNG image"));
    }
    let limit = png::Limits {
        bytes: (MAX_PIXELS * 8) as usize,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(data), limit);
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| bad_mask())?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(bad_mask)?];
    let frame = reader.next_frame(&mut buffer).map_err(|_| bad_mask())?;
    let channels = match frame.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => return Err(bad_mask()),
    };
    let count = width as usize * height as usize;
    let pixels = buffer.get(..frame.buffer_size()).ok_or_else(bad_mask)?;
    if frame.bit_depth != BitDepth::Eight || pixels.len() < count * channels {
        return Err(bad_mask());
    }
    let pixels: Vec<&[u8]> = pixels.chunks_exact(channels).take(count).collect();
    let has_alpha = channels % 2 == 0;
    let alpha = |p: &[u8]| p.last().copied().unwrap_or(255);
    let by_alpha = has_alpha && pixels.iter().any(|p| alpha(p) < 255);
    let gray: Vec<u8> = pixels
        .iter()
        .map(|p| {
            let paint = if by_alpha {
                alpha(p) < 128
            } else {
                let c = |i: usize| u32::from(p.get(i).copied().unwrap_or(0));
                let luma = if channels >= 3 {
                    (c(0) * 299 + c(1) * 587 + c(2) * 114) / 1000
                } else {
                    c(0)
                };
                luma > 127
            };
            if paint { 255 } else { 0 }
        })
        .collect();
    if !gray.contains(&255) {
        return Err(AiError::invalid("the mask marks nothing to fill"));
    }
    Ok((encode_gray(&gray, width, height)?, width, height))
}

fn encode_gray(gray: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AiError> {
    fn fail<E>(_: E) -> AiError {
        AiError::new(ErrorKind::Internal, "the mask could not be prepared")
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(gray).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
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
    let source = inline(request, InputRole::Source, "source image")?;
    let media = sniff(source)
        .ok_or_else(|| AiError::invalid("the source image is not a PNG, JPEG or WebP image"))?;
    let (mask, width, height) = bfl_mask(inline(request, InputRole::Mask, "mask")?)?;
    if media == "image/png" {
        let (w, h) = png_size(source)
            .ok_or_else(|| AiError::invalid("the source image is not a readable PNG"))?;
        check_size(w, h)?;
        if (w, h) != (width, height) {
            return Err(AiError::invalid(
                "the mask must be the same size as the source image",
            ));
        }
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
