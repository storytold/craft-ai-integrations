//! Shared request helpers for scripted ComfyUI media tests.

use craft_ai_comfyui::ComfyProvider;
use craft_ai_core::payload::Payload;
use craft_ai_core::{AiError, JobId, ModelId, Provider, Request};

pub fn submit(p: &ComfyProvider, model: &str, request: &Request) -> Result<JobId, AiError> {
    p.submit(&ModelId::new(model).unwrap(), request)
}

pub fn file_handle(path: &std::path::Path, media: &str) -> Payload {
    let uri = url::Url::from_file_path(path).unwrap().to_string();
    Payload::handle(&uri, media, None).unwrap()
}

pub fn invalid_image_cases(
    secrets: &std::path::Path,
    wrong_kind: &std::path::Path,
    empty: &std::path::Path,
    big: Option<&std::path::Path>,
) -> Vec<(&'static str, Payload)> {
    let mut cases = vec![
        (
            "a text file declared as png",
            file_handle(secrets, "image/png"),
        ),
        (
            "jpeg bytes declared as png",
            file_handle(wrong_kind, "image/png"),
        ),
        ("an empty file", file_handle(empty, "image/png")),
    ];
    if let Some(big) = big {
        cases.push(("a file over the limit", file_handle(big, "image/png")));
    }
    cases.extend([
        (
            "inline text declared as png",
            Payload::inline("image/png", b"the password is hunter2".to_vec()).unwrap(),
        ),
        (
            "inline html declared as jpeg",
            Payload::inline("image/jpeg", b"<html></html>".to_vec()).unwrap(),
        ),
        (
            "a format ComfyUI would receive mislabelled",
            Payload::inline("image/webp", b"RIFF\0\0\0\0WEBPVP8 ".to_vec()).unwrap(),
        ),
    ]);
    cases
}
