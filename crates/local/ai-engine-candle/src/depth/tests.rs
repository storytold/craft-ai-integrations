use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_engine::testing::check_runner;
use serde_json::json;

use super::*;

/// A stand-in network: depth is the horizontal position, at a size of its own choosing, and it
/// counts its passes. The real network is covered by `tests/depth_live.rs` and the zero-weights
/// test in `net`.
struct Ramp {
    passes: Arc<AtomicUsize>,
}

impl Estimate for Ramp {
    fn estimate(&self, _rgb: &[f32], w: u32, h: u32) -> Result<DepthMap, AiError> {
        self.passes.fetch_add(1, Ordering::SeqCst);
        let (w, h) = (w.min(64), h.min(48));
        let data = (0..w * h)
            .map(|i| (i % w) as f32 / (w - 1).max(1) as f32)
            .collect();
        Ok(DepthMap {
            width: w,
            height: h,
            data,
        })
    }
}

/// A runner whose model is the stand-in: the same provider path, no weights to read.
struct StandIn {
    passes: Arc<AtomicUsize>,
}

impl ModelRunner for StandIn {
    fn engine(&self) -> &str {
        DepthRunner.engine()
    }
    fn model(&self) -> ModelInfo {
        DepthRunner.model()
    }
    fn load(&self, _: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        Ok(Box::new(Loaded::new(Box::new(Ramp {
            passes: self.passes.clone(),
        }))))
    }
}

fn png_of(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Fast);
    let mut writer = e.write_header().unwrap();
    writer
        .write_image_data(&vec![120u8; (w * h * 3) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

fn request(w: u32, h: u32) -> Request {
    Request::new(CapabilityId::parse("image.depth").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/png", png_of(w, h)).unwrap(),
    )
}

/// The PNG of a run's one output as (width, height, 16-bit values), after checking that there is
/// exactly one, inline, with no label.
fn one_map(out: &[Output]) -> (u32, u32, Vec<u16>) {
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].label, None);
    let Payload::Inline { media_type, data } = &out[0].payload else {
        panic!("not inline");
    };
    assert_eq!(media_type.as_str(), "image/png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!(
        (frame.color_type, frame.bit_depth),
        (png::ColorType::Grayscale, png::BitDepth::Sixteen)
    );
    let values = buf[..frame.buffer_size()]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    (frame.width, frame.height, values)
}

fn stand_in() -> (Loaded, Arc<AtomicUsize>) {
    let passes = Arc::new(AtomicUsize::new(0));
    (
        Loaded::new(Box::new(Ramp {
            passes: passes.clone(),
        })),
        passes,
    )
}

/// The runner is the Candle route the catalogue names, and it takes one picture.
#[test]
fn it_is_the_candle_depth_anything_the_catalogue_names() {
    let route = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "estimate-depth")
        .and_then(|f| f.locals.into_iter().next())
        .and_then(|l| l.native.into_iter().next())
        .unwrap();
    assert_eq!(DepthRunner.engine(), route.provider);
    let info = DepthRunner.model();
    assert_eq!(info.id.as_str(), route.model);
    let spec = info
        .spec_for(&CapabilityId::parse("image.depth").unwrap())
        .unwrap();
    assert!(spec.requires.contains(&InputRole::Source));
    // The weights file the catalogue records is the head, with the backbone beside it.
    let names: Vec<&str> = route.models.iter().map(|m| m.filename.as_str()).collect();
    assert_eq!(names, [HEAD_FILE, BACKBONE_FILE]);
    assert_eq!(route.weights().unwrap().filename, HEAD_FILE);
}

#[test]
fn it_is_offered_in_the_engine_runner_list() {
    let ids: Vec<String> = crate::runners()
        .iter()
        .map(|r| r.model().id.as_str().to_owned())
        .collect();
    assert_eq!(ids, ["sam3", "depth-anything-v2-small"]);
}

/// The stand-in goes through the real provider and the shared conformance suite.
#[test]
fn passes_the_shared_engine_checks() {
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join(HEAD_FILE);
    std::fs::write(&weights, b"x").unwrap();
    let bad = Request::new(CapabilityId::parse("image.depth").unwrap());
    let runner = StandIn {
        passes: Arc::default(),
    };
    check_runner(
        Arc::new(runner),
        &weights,
        &request(64, 48),
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}

#[test]
fn the_map_comes_back_16_bit_at_the_size_of_the_picture() {
    let (mut model, _) = stand_in();
    for quality in ["fast", "high"] {
        let r = request(150, 90).with_param("quality", json!(quality));
        let out = model.run(&r, &RunContext::detached()).unwrap();
        let (w, h, values) = one_map(&out);
        assert_eq!((w, h), (150, 90), "{quality}");
        // White is near: the stand-in's ramp is nearest on the right, and fills the range.
        assert_eq!(values.iter().copied().min(), Some(0), "{quality}");
        assert_eq!(values.iter().copied().max(), Some(65_535), "{quality}");
        assert!(values[149] > values[0], "{quality}");
    }
}

/// `fast` is one pass; `high` adds a pass per tile on a picture big enough to need them, and is
/// the default.
#[test]
fn fast_is_one_pass_and_high_tiles_a_large_picture() {
    let (mut model, passes) = stand_in();
    let big = request(1200, 900);
    let fast = big.clone().with_param("quality", json!("fast"));
    model.run(&fast, &RunContext::detached()).unwrap();
    assert_eq!(passes.load(Ordering::SeqCst), 1);

    passes.store(0, Ordering::SeqCst);
    model.run(&big, &RunContext::detached()).unwrap();
    let tiles = tiles::plan_tiles(1200, 900, MAX_TILES).len();
    assert!(tiles > 1);
    assert_eq!(
        passes.load(Ordering::SeqCst),
        tiles + 1,
        "the default is high"
    );

    // A picture the network sees whole needs no tiles, in either quality.
    passes.store(0, Ordering::SeqCst);
    model
        .run(&request(300, 200), &RunContext::detached())
        .unwrap();
    assert_eq!(passes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_quality_that_is_not_fast_or_high_is_refused() {
    let (mut model, passes) = stand_in();
    for bad in [json!("best"), json!(3), json!(null), json!("FAST")] {
        let r = request(8, 8).with_param("quality", bad);
        let e = model.run(&r, &RunContext::detached()).err().unwrap();
        assert_eq!(e.kind, ErrorKind::InvalidRequest);
        assert!(e.message.contains("quality"), "{}", e.message);
    }
    assert_eq!(passes.load(Ordering::SeqCst), 0, "refused before any work");
}

#[test]
fn what_is_not_a_picture_inline_is_refused_in_plain_words() {
    let (mut model, _) = stand_in();
    let capability = CapabilityId::parse("image.depth").unwrap();
    let text = Request::new(capability.clone()).with_input(
        InputRole::Source,
        Payload::inline("image/png", b"nope".to_vec()).unwrap(),
    );
    let e = model.run(&text, &RunContext::detached()).err().unwrap();
    assert!(e.message.contains("PNG or a JPEG"), "{}", e.message);
    let link = Request::new(capability.clone()).with_input(
        InputRole::Source,
        Payload::handle("file:///tmp/x.png", "image/png", None).unwrap(),
    );
    let e = model.run(&link, &RunContext::detached()).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("not a link"), "{}", e.message);
    let none = Request::new(capability);
    assert!(model.run(&none, &RunContext::detached()).is_err());
}

#[test]
fn a_stand_in_that_fails_fails_the_run_with_its_words() {
    struct Broken;
    impl Estimate for Broken {
        fn estimate(&self, _: &[f32], _: u32, _: u32) -> Result<DepthMap, AiError> {
            Err(backend("The depth model failed: boom"))
        }
    }
    let mut model = Loaded::new(Box::new(Broken));
    let e = model
        .run(&request(8, 8), &RunContext::detached())
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Provider);
    assert!(e.message.contains("boom"));
}

#[test]
fn weights_without_their_backbone_say_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let head = dir.path().join(HEAD_FILE);
    std::fs::write(&head, b"x").unwrap();
    let e = DepthRunner.load(&head).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(
        e.message.contains(BACKBONE_FILE) && e.message.contains("Set up AI"),
        "{}",
        e.message
    );
    // A weights path with nothing above it cannot have a backbone beside it either.
    assert!(DepthRunner.load(Path::new("")).is_err());
}

#[test]
fn files_that_are_not_the_model_fail_to_load_in_words() {
    let dir = tempfile::tempdir().unwrap();
    let head = dir.path().join(HEAD_FILE);
    std::fs::write(&head, b"not safetensors").unwrap();
    std::fs::write(dir.path().join(BACKBONE_FILE), b"not safetensors either").unwrap();
    let e = DepthRunner.load(&head).err().unwrap();
    assert!(e.message.contains("depth model"), "{}", e.message);
    // An empty file is not the one Set up AI downloads.
    std::fs::write(&head, b"").unwrap();
    let e = DepthRunner.load(&head).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("Set up AI"));
}

/// The load takes both files at 32 bits plus the activations; without the pair it says nothing.
#[test]
fn memory_needed_counts_both_files_at_32_bits_plus_activations() {
    let dir = tempfile::tempdir().unwrap();
    let head = dir.path().join(HEAD_FILE);
    std::fs::write(&head, vec![0u8; 1000]).unwrap();
    assert_eq!(DepthRunner.memory_needed(&head), None);
    std::fs::write(dir.path().join(BACKBONE_FILE), vec![0u8; 500]).unwrap();
    assert_eq!(
        DepthRunner.memory_needed(&head),
        Some(2 * 1500 + ACTIVATION_BYTES)
    );
}
