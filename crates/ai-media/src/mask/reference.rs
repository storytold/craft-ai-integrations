//! Reference pictures for a fill: counted, sized and checked before anything is sent.
//!
//! A reference is an ordinary picture the person attaches to guide what the fill paints (a
//! product, a style, a material). It stays apart from the source and the mask: an adapter sends it
//! after them, in the order the request lists them, and its instruction says what it is for (see
//! [`super::fill_instruction_with_references`]).
//!
//! Invariants: every check here runs before an adapter uploads or sends anything, so a refused
//! reference means nothing left this computer. Each picture is decoded in full, so a broken file
//! is refused here and not by the service after it has been paid for.

use super::Bounds;
use craft_ai_core::{AiError, InputRole, Payload, Request};

/// What one adapter accepts as references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceRule {
    /// The most references one request may carry. Zero means the adapter takes none.
    pub max_count: usize,
    /// The largest file accepted for one reference, in bytes.
    pub max_bytes: usize,
    /// Whether only PNG is accepted (an adapter that uploads every picture as PNG).
    pub png_only: bool,
}

/// One checked reference, in the order the request lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference<'a> {
    pub data: &'a [u8],
    /// `image/png`, `image/jpeg` or `image/webp`, read from the bytes, not the label.
    pub media: &'static str,
}

/// The request's references in request order, each checked against `rule` and `bounds`: the count,
/// inline data only, the byte size, the format read from the bytes, the size in pixels and a full
/// decode. The first failure is returned and nothing is sent.
pub fn references<'a>(
    request: &'a Request,
    bounds: Bounds,
    rule: ReferenceRule,
) -> Result<Vec<Reference<'a>>, AiError> {
    let service = bounds.service;
    let inputs: Vec<&Payload> = request
        .inputs
        .iter()
        .filter(|i| i.role == InputRole::Reference)
        .map(|i| &i.payload)
        .collect();
    if inputs.len() > rule.max_count {
        return Err(AiError::unsupported(if rule.max_count == 0 {
            format!("{service} takes no reference pictures")
        } else {
            format!(
                "{service} takes at most {} reference pictures",
                rule.max_count
            )
        }));
    }
    inputs
        .into_iter()
        .enumerate()
        .map(|(i, payload)| checked(payload, i + 1, bounds, rule))
        .collect()
}

/// Checks reference number `n` (counted from 1, for the error a person reads).
fn checked(
    payload: &Payload,
    n: usize,
    bounds: Bounds,
    rule: ReferenceRule,
) -> Result<Reference<'_>, AiError> {
    let Payload::Inline { data, .. } = payload else {
        return Err(AiError::unsupported(format!(
            "reference picture {n} must be inline image data; this adapter does not read files or URLs"
        )));
    };
    if data.len() > rule.max_bytes {
        return Err(AiError::invalid(format!(
            "reference picture {n} is larger than {} MB",
            rule.max_bytes / (1024 * 1024)
        )));
    }
    let unreadable = || AiError::invalid(format!("reference picture {n} could not be read"));
    let media = match crate::sniff::media_type(data) {
        Some((m @ ("image/png" | "image/jpeg" | "image/webp"), _)) => m,
        _ => {
            return Err(AiError::invalid(format!(
                "reference picture {n} is not a PNG, JPEG or WebP image"
            )));
        }
    };
    if rule.png_only && media != "image/png" {
        return Err(AiError::invalid(format!(
            "reference picture {n} must be a PNG image for {}",
            bounds.service
        )));
    }
    let (width, height) = crate::image_size(data).ok_or_else(unreadable)?;
    bounds.check(width, height)?;
    // A full decode, with the size checked first so a hostile header cannot make it allocate.
    if crate::decoded_result(data) != Some(media) {
        return Err(unreadable());
    }
    Ok(Reference { data, media })
}
