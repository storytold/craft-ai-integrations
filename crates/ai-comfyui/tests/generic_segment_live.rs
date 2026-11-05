//! The generic `image.segment` route against a real local ComfyUI with SAM 2.1 and the open
//! Florence-2-large detector. Live only: set `CRAFT_TEST_COMFYUI_URL` (for example
//! `http://127.0.0.1:8000`) and `CRAFT_TEST_SAM21_IMAGE` (the astronaut sample), then run with
//! `--ignored`. Leaves nothing in the server's `input/` or `output/` folders: the picture goes to
//! the scratch area and the mask is a preview.
use std::io::Cursor;
use std::time::Duration;

use craft_ai_comfyui::{Binding, ComfyConfig, ComfyProvider, Workflow};
use craft_ai_core::capability::CapabilityId;
use craft_ai_core::job::{Event, JobState, Output};
use craft_ai_core::model::InputRole;
use craft_ai_core::payload::Payload;
use craft_ai_core::segment;
use craft_ai_core::{JobId, Provider, Request};
use serde_json::json;

/// Florence-2 finds the subject named in the prompt and SAM 2.1 cuts it out.
fn graph() -> serde_json::Value {
    json!({
        "1":{"class_type":"LoadImage","inputs":{"image":"placeholder.png"}},
        "2":{"class_type":"Florence2ModelLoader","inputs":{"model":craft_ai_comfyui::FLORENCE2_DETECTOR,"precision":"fp16","convert_to_safetensors":false}},
        "3":{"class_type":"Florence2Run","inputs":{"image":["1",0],"florence2_model":["2",0],"text_input":"placeholder","task":"caption_to_phrase_grounding","fill_mask":true,"keep_model_loaded":true}},
        "4":{"class_type":"Florence2toCoordinates","inputs":{"data":["3",3],"index":"","batch":false}},
        "5":{"class_type":"DownloadAndLoadSAM2Model","inputs":{"model":"sam2.1_hiera_large.safetensors","segmentor":"single_image","device":"cuda","precision":"fp16"}},
        "6":{"class_type":"Sam2Segmentation","inputs":{"sam2_model":["5",0],"image":["1",0],"keep_model_loaded":true,"bboxes":["4",1],"individual_objects":false}},
        "7":{"class_type":"MaskToImage","inputs":{"mask":["6",0]}},
        "8":{"class_type":"SaveImage","inputs":{"images":["7",0],"filename_prefix":"craft-generic-segment"}}
    })
}

/// The selection workflow as an app or the library would import it.
fn workflow() -> Workflow {
    Workflow::import_segment(
        "sam21-generic",
        graph(),
        Binding::new("3", "Florence2Run", "text_input"),
        "8",
        Binding::new("1", "LoadImage", "image"),
    )
    .unwrap()
}

/// A selection request for `subject` on the picture.
fn request(image: &[u8], subject: &str) -> Request {
    Request::new(CapabilityId::parse("image.segment").unwrap())
        .with_input(InputRole::Prompt, Payload::text(subject))
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", image.to_vec()).unwrap(),
        )
}

/// Polls until the job ends and returns what it produced, or panics on failure or a timeout.
fn outputs_of(provider: &ComfyProvider, id: &JobId) -> Vec<Output> {
    let mut outputs = Vec::new();
    for _ in 0..1600 {
        let snap = provider.poll(id).unwrap();
        if let JobState::Failed { error } = &snap.state {
            panic!("generic route failed: {error:?}");
        }
        outputs.extend(snap.events.into_iter().filter_map(|e| match e {
            Event::Output(o) => Some(o),
            _ => None,
        }));
        if matches!(snap.state, JobState::Succeeded) {
            return outputs;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    panic!("timed out");
}

/// The share of the mask that is selected, and its size.
fn selected(outputs: &[Output]) -> (f64, (u32, u32)) {
    let Some(Payload::Inline { data, .. }) = segment::mask(outputs) else {
        panic!("an inline mask is expected");
    };
    let mut reader = png::Decoder::new(Cursor::new(data)).read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buffer).unwrap();
    let lit = buffer[..frame.buffer_size()]
        .iter()
        .filter(|v| **v > 127)
        .count();
    (
        lit as f64 / frame.buffer_size() as f64,
        (frame.width, frame.height),
    )
}

/// The route declares a selection model that needs a picture.
#[test]
fn the_route_declares_a_segment_model_that_needs_a_source() {
    let info = workflow().info();
    let spec = info
        .spec_for(&CapabilityId::parse("image.segment").unwrap())
        .unwrap();
    assert!(spec.requires.contains(&InputRole::Source));
}

/// A subject that is there is selected at the picture's size and reported as found, a second
/// selection on the same picture goes through, and a subject that is not there gives a result
/// that agrees with its mask and a job that still succeeds.
#[test]
#[ignore = "requires a running local ComfyUI with SAM 2.1 and Florence-2-large"]
fn the_generic_route_selects_and_keeps_the_result_consistent_with_the_mask() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("local server address");
    let image =
        std::fs::read(std::env::var("CRAFT_TEST_SAM21_IMAGE").expect("test photo path")).unwrap();
    let provider = ComfyProvider::new(
        ComfyConfig::new(url)
            .with_targeted_interrupt(true)
            .with_workflow(workflow()),
    )
    .unwrap();
    let model = provider.models().unwrap()[0].id.clone();
    for subject in ["astronaut", "astronaut"] {
        let id = provider.submit(&model, &request(&image, subject)).unwrap();
        let outputs = outputs_of(&provider, &id);
        let (share, (w, h)) = selected(&outputs);
        println!("{subject}: {w}x{h} selected {share:.3}");
        assert_eq!((w, h), (512, 512));
        assert!((0.02..0.9).contains(&share), "share {share}");
        assert!(segment::read_found(&outputs).unwrap().matched);
    }
    let id = provider
        .submit(&model, &request(&image, "a purple elephant"))
        .unwrap();
    let outputs = outputs_of(&provider, &id);
    let (share, _) = selected(&outputs);
    println!("elephant: selected {share:.3}");
    assert_eq!(
        segment::read_found(&outputs).unwrap().matched,
        share > 0.0,
        "the result agrees with the mask"
    );
}
