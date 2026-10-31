//! p4: a `video.generate` workflow that binds its start image (a Wan 2.2 TI2V or I2V graph loads its
//! first frame through `LoadImage`). The image is required once bound, read and checked by its bytes
//! like an inpaint source, and uploaded before the graph is queued. Scripted localhost servers and
//! temporary folders only.
mod media_support;
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_comfyui::{ComfyConfig, ComfyProvider, Workflow, WorkflowLibrary, WorkflowSpec};
use craft_ai_core::capability::CapabilityId;
use craft_ai_core::model::InputRole;
use craft_ai_core::payload::Payload;
use craft_ai_core::{ErrorKind, Provider, Request};
use media_support::file_handle;
use serde_json::{Value, json};
use support::{Reply, Server};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage body";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF image body";

fn graph() -> Value {
    json!({
        "1":{"class_type":"CLIPTextEncode","inputs":{"text":"old"}},
        "2":{"class_type":"LoadImage","inputs":{"image":"saved-start.png"}},
        "3":{"class_type":"Wan22ImageToVideoLatent","inputs":{"start_image":["2",0],"width":1280,"height":704,"length":121}},
        "4":{"class_type":"KSampler","inputs":{"positive":["1",0],"latent_image":["3",0]}},
        "5":{"class_type":"SaveVideo","inputs":{"video":["4",0],"filename_prefix":"video/wan"}}
    })
}

/// A stored `video.generate` spec over [`graph`], with `extra` fields added or replaced.
fn spec(extra: Value) -> Vec<u8> {
    let mut v = json!({
        "id":"wan-ti2v", "capability":"video.generate", "graph":graph(),
        "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
        "output_node":"5"
    });
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::to_vec(&v).unwrap()
}

fn start_image() -> Value {
    json!({"source":{"node":"2","class_type":"LoadImage","input":"image"}})
}

fn workflow(extra: Value) -> Workflow {
    WorkflowSpec::parse(&spec(extra), None)
        .unwrap()
        .to_workflow()
        .unwrap()
}

/// A scripted ComfyUI with the video nodes installed that accepts uploads to the scratch area.
fn video_server() -> Server {
    Server::start(|r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, "{}".into()),
        "/object_info" => Reply::Json(
            200,
            json!({
                "CLIPTextEncode":{"input":{"required":{"text":["STRING",{}]}}},
                "LoadImage":{"input":{"required":{"image":[["saved-start.png"],{"image_upload":true}]}}},
                "Wan22ImageToVideoLatent":{"input":{"required":{"width":["INT",{}],"height":["INT",{}],"length":["INT",{}]},"optional":{"start_image":["IMAGE",{}]}}},
                "KSampler":{"input":{"required":{"positive":["CONDITIONING",{}],"latent_image":["LATENT",{}]}}},
                "SaveVideo":{"input":{"required":{"video":["VIDEO",{}],"filename_prefix":["STRING",{}]}}}
            })
            .to_string(),
        ),
        "/upload/image" => Reply::Json(
            200,
            json!({"name":"returned-start.png","subfolder":"","type":"temp"}).to_string(),
        ),
        "/prompt" => Reply::Json(200, r#"{"prompt_id":"video-job"}"#.into()),
        _ => Reply::Json(404, "{}".into()),
    }
    })
}

fn provider(server: &Server, workflow: Workflow) -> ComfyProvider {
    ComfyProvider::new(
        ComfyConfig::new(server.url.trim_end_matches("/v1"))
            .with_max_upload_bytes(4096)
            .with_workflow(workflow),
    )
    .unwrap()
}

fn video(image: Option<Payload>) -> Request {
    let request = Request::new(CapabilityId::parse("video.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a kite over the sea"));
    match image {
        Some(image) => request.with_input(InputRole::Source, image),
        None => request,
    }
}

fn submit(
    p: &ComfyProvider,
    request: &Request,
) -> Result<craft_ai_core::JobId, craft_ai_core::AiError> {
    media_support::submit(p, "wan-ti2v", request)
}

fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn has(body: &[u8], needle: &[u8]) -> bool {
    body.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_start_image_is_uploaded_before_the_graph_is_queued() {
    let dir = tempfile::tempdir().unwrap();
    let jpeg = write(&dir, "start.jpg", JPEG);
    let cases = [
        (
            "an inline png",
            Payload::inline("image/png", PNG.to_vec()).unwrap(),
            PNG,
            "image/png",
        ),
        (
            "a local jpeg file",
            file_handle(&jpeg, "image/jpeg"),
            JPEG,
            "image/jpeg",
        ),
    ];
    for (why, image, bytes, media) in cases {
        let server = video_server();
        let p = provider(&server, workflow(start_image()));
        submit(&p, &video(Some(image))).unwrap();
        let recorded = server.recorded();
        let sent: Vec<&str> = recorded
            .iter()
            .map(|r| r.path.as_str())
            .filter(|path| matches!(*path, "/upload/image" | "/prompt"))
            .collect();
        assert_eq!(sent, ["/upload/image", "/prompt"], "{why}");
        let upload = recorded.iter().find(|r| r.path == "/upload/image").unwrap();
        assert!(has(&upload.body, bytes), "{why}: the image bytes were sent");
        assert!(has(&upload.body, media.as_bytes()), "{why}");
        assert!(has(&upload.body, b"craft-source-"), "{why}");
        let graph = &recorded
            .iter()
            .find(|r| r.path == "/prompt")
            .unwrap()
            .body_json()["prompt"];
        assert_eq!(
            graph["2"]["inputs"]["image"], "returned-start.png [temp]",
            "{why}"
        );
        assert_eq!(graph["1"]["inputs"]["text"], "a kite over the sea", "{why}");
    }
}

#[test]
fn a_video_workflow_with_a_start_image_refuses_a_request_without_one() {
    let server = video_server();
    let p = provider(&server, workflow(start_image()));
    let models = p.models().unwrap();
    let spec = &models[0].capabilities[0];
    assert_eq!(spec.capability.as_str(), "video.generate");
    assert_eq!(spec.accepts.get(&InputRole::Source), Some(&1));
    assert!(spec.requires.contains(&InputRole::Source));
    let before = server.count();
    let error = submit(&p, &video(None)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidRequest, "{error:?}");
    assert_eq!(server.count(), before, "nothing reaches ComfyUI");
}

#[test]
fn a_start_image_that_is_not_the_image_it_claims_is_never_uploaded() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = write(&dir, "notes.png", b"the password is hunter2");
    let wrong_kind = write(&dir, "photo.png", JPEG);
    let empty = write(&dir, "empty.png", b"");
    let mut big = PNG.to_vec();
    big.resize(5000, 7);
    let big = write(&dir, "big.png", &big);
    let cases = media_support::invalid_image_cases(&secrets, &wrong_kind, &empty, Some(&big));
    for (why, bad) in cases {
        let server = video_server();
        let p = provider(&server, workflow(start_image()));
        let error = submit(&p, &video(Some(bad))).unwrap_err();
        assert!(
            matches!(
                error.kind,
                ErrorKind::InvalidRequest | ErrorKind::Unsupported
            ),
            "{why}: {error:?}"
        );
        assert!(
            !format!("{error:?}").contains("hunter2"),
            "{why}: the file content is not repeated"
        );
        let recorded = server.recorded();
        assert!(
            !recorded.iter().any(|r| r.path == "/upload/image"),
            "{why}: nothing is sent"
        );
        assert!(!recorded.iter().any(|r| r.path == "/prompt"), "{why}");
    }
}

#[test]
fn a_video_workflow_without_the_binding_stays_prompt_only() {
    let server = video_server();
    let p = provider(&server, workflow(json!({})));
    let models = p.models().unwrap();
    let spec = &models[0].capabilities[0];
    assert_eq!(spec.accepts.get(&InputRole::Source), None);
    assert_eq!(spec.requires, [InputRole::Prompt]);
    submit(&p, &video(None)).unwrap();
    let recorded = server.recorded();
    assert!(!recorded.iter().any(|r| r.path == "/upload/image"));
    let graph = &recorded
        .iter()
        .find(|r| r.path == "/prompt")
        .unwrap()
        .body_json()["prompt"];
    assert_eq!(graph["2"]["inputs"]["image"], "saved-start.png");
    let png = Payload::inline("image/png", PNG.to_vec()).unwrap();
    let error = submit(&p, &video(Some(png))).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported, "{error:?}");
}

#[test]
fn a_stored_video_workflow_keeps_its_start_image() {
    let dir = tempfile::tempdir().unwrap();
    let lib = WorkflowLibrary::at(dir.path().join("workflows")).unwrap();
    lib.save(&WorkflowSpec::parse(&spec(start_image()), None).unwrap())
        .unwrap();
    let loaded = lib.load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].capability().as_str(), "video.generate");
    assert!(loaded[0].needs_uploads());
    assert!(
        loaded[0].info().capabilities[0]
            .requires
            .contains(&InputRole::Source)
    );
}

#[test]
fn a_start_image_binding_must_be_a_connected_image_loader() {
    let parse = |extra: Value| WorkflowSpec::parse(&spec(extra), None);
    let mut with_mask = start_image().as_object().unwrap().clone();
    with_mask.insert(
        "mask".into(),
        json!({"node":"2","class_type":"LoadImage","input":"image"}),
    );
    assert!(parse(with_mask.into()).is_err(), "a mask binding");
    let not_a_loader = json!({"source":{"node":"1","class_type":"CLIPTextEncode","input":"text"}});
    assert!(parse(not_a_loader).is_err(), "a text field");
    let mut stray = graph();
    stray.as_object_mut().unwrap().insert(
        "6".into(),
        json!({"class_type":"LoadImage","inputs":{"image":"unused.png"}}),
    );
    let mut unconnected = start_image();
    unconnected["source"]["node"] = json!("6");
    unconnected["graph"] = stray;
    assert!(parse(unconnected).is_err(), "a loader that feeds nothing");
}
