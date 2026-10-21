//! The generic `image.segment` route against a scripted ComfyUI: the mask is checked before an
//! app sees it, and "nothing there" is an answer, not a failure.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_comfyui::{Binding, ComfyConfig, ComfyProvider, Workflow};
use craft_ai_core::capability::CapabilityId;
use craft_ai_core::job::{Event, JobState, Output};
use craft_ai_core::model::InputRole;
use craft_ai_core::payload::Payload;
use craft_ai_core::segment::{self, Click};
use craft_ai_core::{ErrorKind, JobId, ModelId, Provider, Request};
use png::{BitDepth, ColorType};
use serde_json::json;
use support::{Reply, Server, http_bytes};

/// A PNG, `width` by `height`, every sample `value`, in the given colour type.
fn png_of(color: ColorType, width: u32, height: u32, value: u8) -> Vec<u8> {
    let channels = if color == ColorType::Rgb { 3 } else { 1 };
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![value; (width * height) as usize * channels])
        .unwrap();
    writer.finish().unwrap();
    out
}

/// The selection workflow under test: a loader, a text node, a segmenter and a sink.
fn workflow() -> Workflow {
    Workflow::import_segment(
        "pick",
        json!({
            "1":{"class_type":"LoadImage","inputs":{"image":"old.png"}},
            "2":{"class_type":"TextNode","inputs":{"text":"old"}},
            "3":{"class_type":"Segmenter","inputs":{"image":["1",0],"text":["2",0]}},
            "4":{"class_type":"SaveImage","inputs":{"images":["3",0],"filename_prefix":"mask"}}
        }),
        Binding::new("2", "TextNode", "text"),
        "4",
        Binding::new("1", "LoadImage", "image"),
    )
    .unwrap()
}

/// The nodes the scripted server reports as installed.
fn installed() -> String {
    json!({
        "LoadImage":{"input":{"required":{"image":[[],{"image_upload":true}]}}},
        "TextNode":{"input":{"required":{"text":["STRING",{}]}}},
        "Segmenter":{"input":{"required":{"image":["IMAGE",{}],"text":["STRING",{}]}}},
        "SaveImage":{"input":{"required":{"images":["IMAGE",{}],"filename_prefix":["STRING",{}]}}}
    })
    .to_string()
}

/// A selection request for `subject` on `picture`.
fn request(picture: Vec<u8>, subject: &str) -> Request {
    Request::new(CapabilityId::parse("image.segment").unwrap())
        .with_input(InputRole::Prompt, Payload::text(subject))
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", picture).unwrap(),
        )
}

/// What the scripted server counts and answers.
struct Script {
    mask: Vec<u8>,
    uploads: AtomicUsize,
    prompts: AtomicUsize,
}

/// Starts a scripted ComfyUI that follows `script`.
fn start(script: Arc<Script>) -> Server {
    Server::start(move |r| match r.path.as_str() {
        "/system_stats" => Reply::Json(200, "{}".into()),
        "/object_info" => Reply::Json(200, installed()),
        "/upload/image" => {
            let n = script.uploads.fetch_add(1, Ordering::SeqCst) + 1;
            Reply::Json(
                200,
                json!({"name":format!("up-{n}.png"),"subfolder":"","type":"temp"}).to_string(),
            )
        }
        "/prompt" => {
            script.prompts.fetch_add(1, Ordering::SeqCst);
            Reply::Json(200, r#"{"prompt_id":"seg-job"}"#.into())
        }
        "/history/seg-job" => Reply::Json(
            200,
            json!({"seg-job":{"status":{"completed":true,"status_str":"success"},
                "outputs":{"4":{"images":[{"filename":"m.png","subfolder":"","type":"temp"}]}}}})
            .to_string(),
        ),
        "/view?filename=m.png&type=temp" => http_bytes(200, &[], &script.mask),
        "/queue" | "/interrupt" => Reply::Json(200, "{}".into()),
        _ => Reply::Json(404, "{}".into()),
    })
}

/// A script that answers with `mask` as the finished job output.
fn script(mask: Vec<u8>) -> Arc<Script> {
    Arc::new(Script {
        mask,
        uploads: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
    })
}

/// A provider on the scripted server with the selection workflow.
fn provider(server: &Server) -> ComfyProvider {
    ComfyProvider::new(
        ComfyConfig::new(server.url.trim_end_matches("/v1")).with_workflow(workflow()),
    )
    .unwrap()
}

/// Polls until the job ends and returns its state and what it produced.
fn finish(p: &ComfyProvider, id: &JobId) -> (JobState, Vec<Output>) {
    let mut outputs = Vec::new();
    for _ in 0..20 {
        let snap = p.poll(id).unwrap();
        outputs.extend(snap.events.into_iter().filter_map(|e| match e {
            Event::Output(o) => Some(o),
            _ => None,
        }));
        if snap.state.is_terminal() {
            return (snap.state, outputs);
        }
    }
    panic!("the job did not end");
}

/// The id of the selection workflow.
fn model() -> ModelId {
    ModelId::new("pick").unwrap()
}

/// The answer is the mask as a grayscale PNG and then the shared result, and the graph was queued
/// with the scratch name of the upload and a sink that cannot be served from cache.
#[test]
fn a_description_gives_a_mask_and_a_result_in_the_shared_shape() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let id = p
        .submit(
            &model(),
            &request(png_of(ColorType::Rgb, 6, 4, 9), "the dog"),
        )
        .unwrap();
    let (state, outputs) = finish(&p, &id);
    assert!(matches!(state, JobState::Succeeded), "{state:?}");
    assert!(segment::mask(&outputs).is_some());
    let found = segment::read_found(&outputs).unwrap();
    assert!(found.matched);
    let graph = &server
        .recorded()
        .iter()
        .find(|r| r.path == "/prompt")
        .unwrap()
        .body_json()["prompt"];
    assert_eq!(graph["1"]["inputs"]["image"], "up-1.png [temp]");
    assert_eq!(graph["2"]["inputs"]["text"], "the dog");
    assert_eq!(graph["4"]["class_type"], "PreviewImage");
}

/// Nothing selected is `matched: false` with a succeeded job.
#[test]
fn a_black_mask_is_no_match_and_the_job_still_succeeds() {
    let server = start(script(png_of(ColorType::Rgb, 6, 4, 0)));
    let p = provider(&server);
    let id = p
        .submit(
            &model(),
            &request(png_of(ColorType::Rgb, 6, 4, 9), "unicorn"),
        )
        .unwrap();
    let (state, outputs) = finish(&p, &id);
    assert!(matches!(state, JobState::Succeeded), "{state:?}");
    assert!(!segment::read_found(&outputs).unwrap().matched);
    assert!(segment::mask(&outputs).is_some());
}

/// A mask of another size than the picture fails the job and hands on nothing.
#[test]
fn a_mask_of_the_wrong_size_fails_the_job() {
    let server = start(script(png_of(ColorType::Rgb, 5, 5, 255)));
    let p = provider(&server);
    let id = p
        .submit(
            &model(),
            &request(png_of(ColorType::Rgb, 6, 4, 9), "the dog"),
        )
        .unwrap();
    let (state, outputs) = finish(&p, &id);
    let JobState::Failed { error } = state else {
        panic!("a mask of the wrong size must fail");
    };
    assert!(error.message.contains("not the size"), "{}", error.message);
    assert!(outputs.is_empty());
}

/// Each queued graph has a sink of its own, so ComfyUI does not answer a repeat from cache with no
/// output.
#[test]
fn every_selection_has_its_own_sink() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let picture = png_of(ColorType::Rgb, 6, 4, 9);
    for subject in ["the dog", "the cat", "the dog"] {
        p.submit(&model(), &request(picture.clone(), subject))
            .unwrap();
    }
    p.submit(
        &model(),
        &request(png_of(ColorType::Rgb, 6, 4, 10), "the dog"),
    )
    .unwrap();
    let recorded = server.recorded();
    let prefixes: Vec<_> = recorded
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"]["4"]["inputs"]["filename_prefix"].clone())
        .collect();
    assert_eq!(prefixes.len(), 4);
    for (i, a) in prefixes.iter().enumerate() {
        assert!(prefixes[i + 1..].iter().all(|b| a != b), "{prefixes:?}");
    }
}

/// A route that selects by description says so when asked to click, before anything is sent.
#[test]
fn clicks_are_refused_in_plain_words_before_anything_is_sent() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let clicked = segment::with_clicks(
        request(png_of(ColorType::Rgb, 6, 4, 9), "the dog"),
        &[Click {
            x: 0.5,
            y: 0.5,
            positive: true,
        }],
    );
    let e = p.submit(&model(), &clicked).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(
        e.message.contains("describing") && e.message.contains("Set up AI"),
        "{}",
        e.message
    );
    assert_eq!(s.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(s.prompts.load(Ordering::SeqCst), 0);
}

/// With the targeted interrupt confirmed, Cancel removes the queued prompt and interrupts it.
#[test]
fn cancel_removes_the_queued_prompt_and_interrupts_it() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s);
    let p = ComfyProvider::new(
        ComfyConfig::new(server.url.trim_end_matches("/v1"))
            .with_targeted_interrupt(true)
            .with_workflow(workflow()),
    )
    .unwrap();
    let id = p
        .submit(
            &model(),
            &request(png_of(ColorType::Rgb, 6, 4, 9), "the dog"),
        )
        .unwrap();
    p.cancel(&id).unwrap();
    let recorded = server.recorded();
    let queue = recorded.iter().find(|r| r.path == "/queue").unwrap();
    assert_eq!(queue.body_json(), json!({"delete":["seg-job"]}));
    let interrupt = recorded.iter().find(|r| r.path == "/interrupt").unwrap();
    assert_eq!(interrupt.body_json(), json!({"prompt_id":"seg-job"}));
}

/// A stored workflow file can be a selection workflow, with its source binding.
#[test]
fn a_stored_workflow_can_be_a_selection_workflow() {
    let spec = |extra: serde_json::Value| {
        let mut v = json!({
            "id":"pick","capability":"image.segment",
            "graph":{
                "1":{"class_type":"LoadImage","inputs":{"image":"old.png"}},
                "2":{"class_type":"TextNode","inputs":{"text":"old"}},
                "3":{"class_type":"Segmenter","inputs":{"image":["1",0],"text":["2",0]}},
                "4":{"class_type":"SaveImage","inputs":{"images":["3",0],"filename_prefix":"mask"}}
            },
            "prompt":{"node":"2","class_type":"TextNode","input":"text"},
            "output_node":"4"
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::to_vec(&v).unwrap()
    };
    let source = json!({"source":{"node":"1","class_type":"LoadImage","input":"image"}});
    let parse = |bytes: Vec<u8>| craft_ai_comfyui::WorkflowSpec::parse(&bytes, None);
    let workflow = parse(spec(source.clone())).unwrap().to_workflow().unwrap();
    assert_eq!(workflow.capability().as_str(), "image.segment");
    assert!(parse(spec(json!({}))).is_err(), "no source binding");
    let mut with_mask = source.as_object().unwrap().clone();
    with_mask.insert(
        "mask".into(),
        json!({"node":"1","class_type":"LoadImage","input":"image"}),
    );
    assert!(parse(spec(with_mask.into())).is_err(), "a mask binding");
}

/// A selection workflow is only built with its source mapped and connected to the output.
#[test]
fn a_selection_workflow_needs_its_source_mapped_and_connected() {
    let graph = || {
        json!({
            "1":{"class_type":"LoadImage","inputs":{"image":"old.png"}},
            "2":{"class_type":"TextNode","inputs":{"text":"old"}},
            "3":{"class_type":"Segmenter","inputs":{"image":["1",0],"text":["2",0]}},
            "4":{"class_type":"SaveImage","inputs":{"images":["3",0],"filename_prefix":"mask"}},
            "5":{"class_type":"LoadImage","inputs":{"image":"unused.png"}}
        })
    };
    let prompt = || Binding::new("2", "TextNode", "text");
    // No way in without a source binding.
    assert!(Workflow::import("x", "image.segment", graph(), prompt(), "4").is_err());
    // A loader that feeds nothing is not the source.
    let stray = Workflow::import_segment(
        "x",
        graph(),
        prompt(),
        "4",
        Binding::new("5", "LoadImage", "image"),
    );
    assert!(stray.is_err());
}
