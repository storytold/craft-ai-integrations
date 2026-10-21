//! The generic `image.segment` route against a scripted ComfyUI: the picture is sent once, the mask
//! is checked before an app sees it, and "nothing there" is an answer, not a failure.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
    /// How many `/prompt` calls to refuse with a 400 first, as a cleared scratch area would.
    refuse_prompts: AtomicUsize,
    prompts: AtomicUsize,
    /// While set, the server reports every job as failed.
    fail_jobs: AtomicBool,
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
            if script
                .refuse_prompts
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Reply::Json(400, "{}".into());
            }
            Reply::Json(200, r#"{"prompt_id":"seg-job"}"#.into())
        }
        "/history/seg-job" if script.fail_jobs.load(Ordering::SeqCst) => Reply::Json(
            200,
            json!({"seg-job":{"status":{"completed":false,"status_str":"error"},"outputs":{}}})
                .to_string(),
        ),
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
        refuse_prompts: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        fail_jobs: AtomicBool::new(false),
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

/// The same picture is sent once however many selections follow, and each queued graph has a
/// sink of its own so ComfyUI does not answer the second from cache with no output.
#[test]
fn a_picture_is_sent_once_and_every_selection_has_its_own_sink() {
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
    assert_eq!(s.uploads.load(Ordering::SeqCst), 2, "one per picture");
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

/// When ComfyUI no longer has the earlier upload (a restart clears the scratch area), the picture
/// is sent again and the selection still goes through.
#[test]
fn a_cleared_scratch_area_costs_one_more_upload_not_the_selection() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let picture = png_of(ColorType::Rgb, 6, 4, 9);
    p.submit(&model(), &request(picture.clone(), "the dog"))
        .unwrap();
    s.refuse_prompts.store(1, Ordering::SeqCst);
    p.submit(&model(), &request(picture, "the cat")).unwrap();
    assert_eq!(s.uploads.load(Ordering::SeqCst), 2);
    assert_eq!(s.prompts.load(Ordering::SeqCst), 3);
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

/// Only the last few pictures are remembered: the oldest is sent again after four others.
#[test]
fn the_oldest_picture_is_forgotten_after_four_others() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let picture = |v| png_of(ColorType::Rgb, 6, 4, v);
    for v in 1..=5 {
        p.submit(&model(), &request(picture(v), "the dog")).unwrap();
    }
    assert_eq!(s.uploads.load(Ordering::SeqCst), 5);
    p.submit(&model(), &request(picture(5), "the dog")).unwrap();
    assert_eq!(
        s.uploads.load(Ordering::SeqCst),
        5,
        "the newest is remembered"
    );
    p.submit(&model(), &request(picture(1), "the dog")).unwrap();
    assert_eq!(
        s.uploads.load(Ordering::SeqCst),
        6,
        "the oldest was dropped"
    );
}

/// A picture that was just sent is not sent twice when the server refuses the job: the refusal is
/// the person's to see, not something to retry.
#[test]
fn a_refusal_for_a_fresh_upload_is_not_retried() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    s.refuse_prompts.store(1, Ordering::SeqCst);
    let server = start(s.clone());
    let p = provider(&server);
    let e = p
        .submit(
            &model(),
            &request(png_of(ColorType::Rgb, 6, 4, 9), "the dog"),
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert_eq!(s.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(s.prompts.load(Ordering::SeqCst), 1);
}

/// After a job fails, the next selection sends the picture again, since the file may be what went
/// wrong.
#[test]
fn a_failed_job_makes_the_next_selection_send_the_picture_again() {
    let s = script(png_of(ColorType::Rgb, 6, 4, 255));
    let server = start(s.clone());
    let p = provider(&server);
    let picture = png_of(ColorType::Rgb, 6, 4, 9);
    s.fail_jobs.store(true, Ordering::SeqCst);
    let id = p
        .submit(&model(), &request(picture.clone(), "the dog"))
        .unwrap();
    let (state, _) = finish(&p, &id);
    assert!(matches!(state, JobState::Failed { .. }), "{state:?}");
    s.fail_jobs.store(false, Ordering::SeqCst);
    p.submit(&model(), &request(picture, "the dog")).unwrap();
    assert_eq!(s.uploads.load(Ordering::SeqCst), 2);
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
