//! A FLUX Fill result with a good PNG signature but a broken body fails the job, rather than
//! finishing as a success and failing later when the app imports it.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_bfl::{BflConfig, BflProvider};
use craft_ai_core::{
    CapabilityId, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use serde_json::json;
use support::{Reply, Server, http_bytes};

/// An 8-bit grayscale PNG of one shade.
fn gray_png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![shade; (width * height) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

/// A server whose task is Ready at once and whose download answers `picture`.
fn ready_with(picture: Vec<u8>) -> Server {
    Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/flux-pro-1.0-fill") => {
            Reply::Json(200, r#"{"id":"fill-1","polling_url":"ignored"}"#.into())
        }
        ("GET", "/v1/get_result?id=fill-1") => {
            let url = format!("http://{}/delivery/sample.png", r.header("host").unwrap());
            Reply::Json(
                200,
                json!({"id":"fill-1","status":"Ready","result":{"sample":url}}).to_string(),
            )
        }
        ("GET", "/delivery/sample.png") => {
            http_bytes(200, &[("Content-Type", "image/png")], &picture)
        }
        _ => Reply::Json(404, "{}".into()),
    })
}

/// The state of a fill whose result download answers `picture`.
fn finished_with(picture: Vec<u8>) -> JobState {
    let s = ready_with(picture);
    let p = BflProvider::new(BflConfig::new(
        &s.url,
        SecretString::new("fixture-only-key"),
    ))
    .unwrap();
    let mask = Payload::inline("image/png", gray_png(8, 8, 255)).unwrap();
    let request = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", gray_png(8, 8, 128)).unwrap(),
        )
        .with_input(InputRole::Mask, mask)
        .with_input(InputRole::Prompt, Payload::text("a red door"));
    let model = ModelId::new("flux-pro-1.0-fill").unwrap();
    let id = p.submit(&model, &request).unwrap();
    p.poll(&id).unwrap().state
}

/// A whole PNG succeeds, and the same PNG cut in half fails the job.
#[test]
fn a_broken_result_fails_the_job() {
    let whole = gray_png(32, 32, 200);
    assert_eq!(finished_with(whole.clone()), JobState::Succeeded);
    let cut = whole[..whole.len() / 2].to_vec();
    assert!(matches!(finished_with(cut), JobState::Failed { .. }));
}
