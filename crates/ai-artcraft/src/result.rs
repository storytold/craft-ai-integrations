//! Existing linked outputs and immediate checked image delivery for fills.
use crate::http::Http;
use craft_ai_core::{AiError, ErrorKind, Payload, endpoint::Endpoint};

/// Fill results are downloaded without the API key. Other outputs retain the existing contract.
pub(crate) fn payload(
    http: &Http,
    uri: &str,
    video: bool,
    inline: bool,
) -> Result<Payload, AiError> {
    if inline {
        return http.image(uri);
    }
    let endpoint = Endpoint::parse(uri)
        .map_err(|_| AiError::new(ErrorKind::Provider, "ArtCraft job result URL is invalid"))?;
    if !endpoint.https || endpoint.is_loopback() || uri.contains('@') {
        return Err(AiError::new(
            ErrorKind::Provider,
            "ArtCraft job result URL is unsafe",
        ));
    }
    let path = uri.split('?').next().unwrap_or(uri).to_ascii_lowercase();
    let media = if video {
        if path.ends_with(".mp4") {
            "video/mp4"
        } else if path.ends_with(".webm") {
            "video/webm"
        } else {
            return Err(AiError::unsupported(
                "ArtCraft returned an unknown video format",
            ));
        }
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else {
        return Err(AiError::unsupported(
            "ArtCraft returned an unknown image format",
        ));
    };

    Payload::handle(uri, media, None)
}
