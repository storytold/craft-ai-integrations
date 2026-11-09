//! Real Omni adapter with scripted uploads, generation, polling and image delivery.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::{Reply, Server};

/// A real PNG with an editable opaque-white mask, no personal image data.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::new();
    {
        let mut e = png::Encoder::new(&mut data, width, height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .unwrap()
            .write_image_data(&vec![255; (width * height * 4) as usize])
            .unwrap();
    }
    data
}

/// The same three inputs PhotoCraft submits through the shared catalogue role.
fn request() -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png(8, 8)).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png(8, 8)).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a small red balloon"))
}

/// A complete binary response, used to prove result bytes rather than a URL reach the app.
fn bytes(data: &[u8]) -> Reply {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        data.len()
    )
    .into_bytes();
    response.extend_from_slice(data);
    Reply::Raw(response)
}

/// Actual API-key adapter; only its address points at our scripted server.
fn provider(server: &Server) -> ArtCraftProvider {
    ArtCraftProvider::new(ArtCraftConfig::new(
        server.url.trim_end_matches("/v1"),
        SecretString::new("fixture-only-key"),
    ))
    .unwrap()
}

/// Uploads must use API-key auth, fresh UUIDs, private intermediate files and a fixed filename.
fn check_upload(r: &support::Recorded) {
    assert_eq!(r.header("authorization"), Some("Bearer fixture-only-key"));
    assert!(r.header("cookie").is_none());
    assert!(
        r.header("content-type")
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    let body = r.body_text();
    for field in [
        "uuid_idempotency_token",
        "file",
        "maybe_visibility",
        "is_intermediate_system_file",
    ] {
        assert!(body.contains(&format!("name=\"{field}\"")), "{field}");
    }
    assert!(body.contains("\r\n\r\nprivate\r\n"));
    assert!(body.contains("\r\n\r\ntrue\r\n"));
    assert!(body.contains("filename=\"craft-fill.png\""));
}

/// A successful real-adapter lifecycle with no credentials sent to the result host.
#[test]
fn nano_banana_2_uploads_both_inputs_and_delivers_inline_image_once() {
    let uploads = Arc::new(AtomicUsize::new(0));
    let n = uploads.clone();
    let server = Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/omni_api/upload/image") => {
            check_upload(r);
            n.fetch_add(1, Ordering::SeqCst);
            // The token names what was uploaded, not the order it came in, so sending the mask
            // first would put the tokens in the wrong order and fail the generate check below.
            // The source goes up as it was given (RGBA); the mask goes up normalised to grayscale.
            let source = png(8, 8);
            let is_source = r.body.windows(source.len()).any(|w| w == source.as_slice());
            let token = if is_source { "m_source" } else { "m_mask" };
            Reply::Json(
                200,
                format!(r#"{{"success":true,"media_file_token":"{token}"}}"#),
            )
        }
        ("POST", "/v1/omni_api/generate/image") => {
            let body = r.body_json();
            assert_eq!(body["model"], "nano_banana_2");
            assert_eq!(
                body["image_media_tokens"],
                serde_json::json!(["m_source", "m_mask"])
            );
            assert_eq!(body["image_batch_count"], 1);
            assert!(body["prompt"].as_str().unwrap().contains("white"));
            assert!(
                body["prompt"]
                    .as_str()
                    .unwrap()
                    .contains("a small red balloon")
            );
            Reply::Json(
                200,
                r#"{"success":true,"inference_job_token":"jinf_fill"}"#.into(),
            )
        }
        ("GET", "/v1/omni_api/job_status/job/jinf_fill") => {
            let host = r.header("host").unwrap();
            Reply::Json(
                200,
                serde_json::json!({"success":true,"state":{"job_token":"jinf_fill",
                "status":{"status":"complete_success"},"maybe_result":{"media_links":{
                    "cdn_url":format!("http://{host}/result.png")}}}})
                .to_string(),
            )
        }
        ("GET", "/result.png") => {
            assert!(r.header("authorization").is_none());
            assert!(r.header("cookie").is_none());
            bytes(&png(8, 8))
        }
        _ => Reply::Json(500, "unexpected request".into()),
    });
    let p = provider(&server);
    let id = p
        .submit(&ModelId::new("nano_banana_2").unwrap(), &request())
        .unwrap();
    let result = p.poll(&id).unwrap();
    assert_eq!(result.state, JobState::Succeeded);
    assert!(matches!(&result.outputs().next().unwrap().payload,
        Payload::Inline { media_type, data } if media_type.as_str() == "image/png" && *data == png(8, 8)));
    assert!(p.poll(&id).unwrap().events.is_empty());
    assert_eq!(uploads.load(Ordering::SeqCst), 2);
    assert_eq!(server.count(), 5);
    let uploads: Vec<_> = server
        .recorded()
        .into_iter()
        .filter(|r| r.path.ends_with("upload/image"))
        .collect();
    assert_ne!(
        uploads[0].header("content-type"),
        uploads[1].header("content-type")
    );
}

/// Malformed, empty and differently sized masks fail before any upload or paid request.
#[test]
fn invalid_fill_inputs_send_nothing() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = provider(&server);
    let model = ModelId::new("nano_banana_2").unwrap();
    for mask in [vec![1, 2, 3], png(4, 8)] {
        let mut req = request();
        req.inputs[1].payload = Payload::inline("image/png", mask).unwrap();
        assert_eq!(
            p.submit(&model, &req).unwrap_err().kind,
            ErrorKind::InvalidRequest
        );
    }
    let req = request().with_param("unknown", serde_json::json!(true));
    assert_eq!(
        p.submit(&model, &req).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    assert_eq!(server.count(), 0);
}

/// The built-in catalogue must agree with the executable route for every offered Banana model.
#[test]
fn banana_catalogue_exposes_generation_and_mask_guided_fill() {
    let cap = CapabilityId::parse("image.inpaint").unwrap();
    for name in ["nano_banana", "nano_banana_2", "nano_banana_pro"] {
        let model = ArtCraftProvider::documented_models()
            .into_iter()
            .find(|m| m.id.as_str() == name)
            .unwrap();
        let spec = model.spec_for(&cap).unwrap();
        assert!(spec.requires.contains(&InputRole::Source));
        assert!(spec.requires.contains(&InputRole::Mask));
    }
}

/// A hostile delivery answer is refused without redirects or authenticated CDN requests.
#[test]
fn invalid_result_hosts_redirects_pages_and_oversized_images_are_refused() {
    for mode in ["foreign", "redirect", "page", "oversize"] {
        let n = Arc::new(AtomicUsize::new(0));
        let server = Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/v1/omni_api/upload/image") => Reply::Json(
                200,
                format!(
                    r#"{{"success":true,"media_file_token":"m_{}"}}"#,
                    n.fetch_add(1, Ordering::SeqCst)
                ),
            ),
            ("POST", "/v1/omni_api/generate/image") => Reply::Json(
                200,
                r#"{"success":true,"inference_job_token":"jinf_guard"}"#.into(),
            ),
            ("GET", "/v1/omni_api/job_status/job/jinf_guard") => {
                let uri = if mode == "foreign" {
                    "https://example.invalid/result.png".to_owned()
                } else {
                    format!("http://{}/result.png", r.header("host").unwrap())
                };
                Reply::Json(200, serde_json::json!({"success":true,"state":{"job_token":"jinf_guard",
                    "status":{"status":"complete_success"},"maybe_result":{"media_links":{"cdn_url":uri}}}}).to_string())
            }
            ("GET", "/result.png") => {
                assert!(r.header("authorization").is_none());
                match mode {
                    "redirect" => Reply::JsonWith(302, vec![("Location", "/stolen.png".into())], "{}".into()),
                    "oversize" => Reply::Raw(b"HTTP/1.1 200 OK\r\nContent-Length: 33554433\r\nConnection: close\r\n\r\nx".to_vec()),
                    _ => bytes(b"<html>not an image</html>")
                }
            }
            _ => panic!("unexpected follow-up request"),
        });
        let p = provider(&server);
        let id = p
            .submit(&ModelId::new("nano_banana_2").unwrap(), &request())
            .unwrap();
        // None of these will get better on another poll, so the job ends as failed and stops
        // counting as active, rather than returning an error for ever.
        let snap = p.poll(&id).unwrap();
        let JobState::Failed { error } = &snap.state else {
            panic!("{mode}: {:?}", snap.state);
        };
        assert_eq!(error.kind, ErrorKind::Provider, "{mode}");
        if mode == "foreign" {
            assert_eq!(error.message, "ArtCraft result image URL is not permitted");
        }
        let sent = if mode == "foreign" { 4 } else { 5 };
        assert_eq!(server.count(), sent, "{mode}");
        assert!(matches!(
            p.poll(&id).unwrap().state,
            JobState::Failed { .. }
        ));
        assert_eq!(
            server.count(),
            sent,
            "{mode}: a failed job is not asked about again"
        );
    }
}
