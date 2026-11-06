//! Mask-guided Omni editing: validate everything before uploading either input.
//!
//! Omni has reference images, not a dedicated mask field. The normalized mask is the second
//! reference, and the shared instruction explains it. Callers composite only inside the mask.
use craft_ai_core::{AiError, InputRole, Request};
use craft_ai_media::mask::{self, Bounds};

const BOUNDS: Bounds = Bounds {
    service: "ArtCraft Omni fill",
    max_side: 8192,
    max_pixels: 32 * 1024 * 1024,
};

pub(crate) struct Fill<'a> {
    pub source: &'a [u8],
    pub mask: Vec<u8>,
}

/// A PNG source and a normalized, non-empty PNG mask with matching dimensions.
pub(crate) fn prepare(request: &Request) -> Result<Fill<'_>, AiError> {
    let source = mask::inline(request, BOUNDS, InputRole::Source, "source picture")?;
    // Decode the whole source, not only its header, so a broken file never reaches an upload.
    let source_size = mask::png_checked(source, BOUNDS, "Omni fill source")?;
    let original = mask::inline(request, BOUNDS, InputRole::Mask, "mask")?;
    let (mask, width, height) = mask::binary(original, BOUNDS)?;
    if source_size != (width, height) {
        return Err(AiError::invalid(
            "the Omni fill source and mask must be the same size",
        ));
    }
    Ok(Fill { source, mask })
}
