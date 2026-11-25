use std::path::{Path, PathBuf};
use std::sync::Arc;

use burn::prelude::*;
use burn_store::{BurnpackStore, ModuleSnapshot};
use craft_ai_core::{AiError, CapabilityId, ErrorKind, InputRole, ModelInfo, Payload, Request};
use craft_ai_engine::testing::check_runner;
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use craft_ai_engine_rten::picture::{decode_picture, encode_mask};
use craft_ai_features::GpuVendor;

use super::net::InSPyReNet;
use super::net::swin::SwinConfig;
use super::pipeline::Input;
use super::{InspyrenetRunner, Loaded, Predict, on_big_stack, weights};

/// A Swin backbone small enough to build, save and run in a test.
const TINY: SwinConfig = SwinConfig {
    embed_dim: 8,
    depths: [1, 1, 1, 1],
    heads: [1, 1, 1, 1],
    window: 12,
    patch: 4,
};

#[allow(deprecated)]
fn processor() -> Device {
    burn::backend::ndarray::NdArrayDevice::Cpu.into()
}

/// A tiny network with placeholder weights, saved as a Burn copy in `dir`.
fn tiny_weights(dir: &Path) -> PathBuf {
    let path = dir.join("tiny.bpk");
    let target = path.clone();
    on_big_stack(move || {
        let net = InSPyReNet::with_backbone(TINY, &processor());
        net.save_into(&mut BurnpackStore::from_file(&target))
            .map_err(|_| craft_ai_core::AiError::invalid("save"))
    })
    .unwrap();
    path
}

/// The tiny network's runner, on the processor whatever graphics the computer has.
fn tiny_runner() -> Arc<dyn ModelRunner> {
    Arc::new(InspyrenetRunner {
        backbone: TINY,
        device: processor,
    })
}

/// A request for `image.matte` with a gray `w × h` PNG.
fn request(w: usize, h: usize) -> Request {
    let alpha: Vec<f32> = (0..w * h).map(|i| (i % 7) as f32 / 7.0).collect();
    let png = encode_mask(&alpha, w, h).unwrap();
    Request::new(CapabilityId::parse("image.matte").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/png", png).unwrap(),
    )
}

#[test]
fn the_runner_describes_an_image_matte_from_one_picture() {
    let info = InspyrenetRunner::new().model();
    assert_eq!(info.id.as_str(), "inspyrenet");
    assert_eq!(InspyrenetRunner::new().engine(), "burn");
    let spec = info.capabilities.first().unwrap();
    assert_eq!(spec.capability.as_str(), "image.matte");
    assert!(
        crate::runners()
            .iter()
            .any(|r| r.model().id.as_str() == "inspyrenet")
    );
}

/// Stands in for the network: the left half of the network's input is the subject.
struct LeftHalf;

impl Predict for LeftHalf {
    fn predict(&mut self, input: &Input, _: &RunContext) -> Result<Vec<f32>, AiError> {
        Ok((0..input.width * input.height)
            .map(|i| {
                if i % input.width < input.width / 2 {
                    1.0
                } else {
                    0.0
                }
            })
            .collect())
    }
}

/// The real runner's description and request handling around [`LeftHalf`], whatever file is
/// named: the network itself is far too slow for these tests in an unoptimised build.
struct StandIn;

impl ModelRunner for StandIn {
    fn engine(&self) -> &str {
        crate::runner::ENGINE
    }
    fn model(&self) -> ModelInfo {
        InspyrenetRunner::new().model()
    }
    fn accelerators(&self) -> &[GpuVendor] {
        &super::CARDS
    }
    fn load(&self, _: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        Ok(Box::new(Loaded::new(Box::new(LeftHalf))))
    }
}

/// The mask PNG of a run's one output, after checking that there is exactly one.
fn one_mask(out: &[craft_ai_core::Output]) -> craft_ai_engine_rten::picture::Rgb {
    assert_eq!(out.len(), 1);
    let Payload::Inline { media_type, data } = &out[0].payload else {
        panic!("not inline");
    };
    assert_eq!(media_type.as_str(), "image/png");
    decode_picture(data).unwrap()
}

#[test]
fn passes_the_shared_engine_checks() {
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join("inspyrenet.safetensors");
    std::fs::write(&weights, b"x").unwrap();
    let bad = Request::new(CapabilityId::parse("image.matte").unwrap());
    check_runner(
        Arc::new(StandIn),
        &weights,
        &request(40, 30),
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}

#[test]
fn answers_one_mask_at_the_pictures_size() {
    let mut model = StandIn.load(Path::new("unused")).unwrap();
    let mask = one_mask(
        &model
            .run(&request(50, 20), &RunContext::detached())
            .unwrap(),
    );
    assert_eq!((mask.width, mask.height), (50, 20));
    // The subject is the left half: white there, black on the right after the fringe shrink.
    let at = |x: usize| mask.bytes[3 * x];
    assert!(at(5) > 200 && at(45) < 50, "{} {}", at(5), at(45));
}

/// The tiny network itself, through load and run on the processor. Ignored because Burn's
/// processor backend is very slow without optimisation; run it in release.
#[test]
#[ignore = "slow in an unoptimised build: cargo test --release -p craft-ai-engine-burn --lib tiny_network -- --ignored"]
fn the_tiny_network_answers_one_mask_at_the_pictures_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut model = tiny_runner().load(&tiny_weights(dir.path())).unwrap();
    let mask = one_mask(
        &model
            .run(&request(50, 20), &RunContext::detached())
            .unwrap(),
    );
    assert_eq!((mask.width, mask.height), (50, 20));
}

#[test]
fn files_that_are_not_the_checkpoint_are_refused_in_words() {
    let dir = tempfile::tempdir().unwrap();
    let runner = InspyrenetRunner {
        device: processor,
        ..InspyrenetRunner::new()
    };
    let missing = runner
        .load(&dir.path().join("inspyrenet.safetensors"))
        .err()
        .unwrap();
    assert_eq!(missing.kind, ErrorKind::Unsupported);
    for (name, bytes) in [
        ("junk.pth", b"not a checkpoint".as_slice()),
        ("cut.bpk", b"NRUB and then nothing".as_slice()),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let refused = runner.load(&path).err().unwrap();
        assert_eq!(refused.kind, ErrorKind::Unsupported, "{name}");
    }
    // The tiny copy does not fit the Swin-B network: refused, not half loaded.
    let tiny = tiny_weights(dir.path());
    assert_eq!(
        runner.load(&tiny).err().unwrap().kind,
        ErrorKind::Unsupported
    );
}

/// The published checkpoint, for the tests that need the real weights.
fn checkpoint() -> Option<PathBuf> {
    std::env::var_os("CRAFT_TEST_INSPYRENET_WEIGHTS").map(PathBuf::from)
}

#[test]
#[ignore = "needs CRAFT_TEST_INSPYRENET_WEIGHTS (inspyrenet.safetensors)"]
fn the_published_checkpoint_fills_every_weight() {
    let Some(path) = checkpoint() else {
        return;
    };
    let values = on_big_stack(move || {
        let device = processor();
        let net = weights::fill(weights::untrained(&device), &path)?;
        let x = Tensor::<4>::from_data(
            burn::tensor::TensorData::new(vec![0.1f32; 3 * 64 * 64], [1, 3, 64, 64]),
            &device,
        );
        let out = net
            .forward(x, &|| false)
            .ok_or_else(craft_ai_core::AiError::cancelled)?;
        out.into_data()
            .try_to_vec::<f32>()
            .map_err(|_| craft_ai_core::AiError::invalid("unreadable"))
    })
    .unwrap();
    assert_eq!(values.len(), 64 * 64);
    assert!(values.iter().all(|v| v.is_finite()));
}

/// The whole runner on a real photo, as an app calls it: the picture in, the mask PNG out, written
/// to `CRAFT_TEST_INSPYRENET_OUT` for a look. Prints the times.
#[test]
#[ignore = "needs CRAFT_TEST_INSPYRENET_WEIGHTS, CRAFT_TEST_INSPYRENET_IMAGE and CRAFT_TEST_INSPYRENET_OUT"]
fn removes_the_background_of_a_real_photo() {
    let (Some(weights), Some(image), Some(out)) = (
        checkpoint(),
        std::env::var_os("CRAFT_TEST_INSPYRENET_IMAGE"),
        std::env::var_os("CRAFT_TEST_INSPYRENET_OUT"),
    ) else {
        return;
    };
    let picture = std::fs::read(image).unwrap();
    let request = Request::new(CapabilityId::parse("image.matte").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/jpeg", picture).unwrap(),
    );
    let started = std::time::Instant::now();
    let mut model = InspyrenetRunner::new().load(&weights).unwrap();
    println!("load {:.2?}", started.elapsed());
    for round in 0..2 {
        let started = std::time::Instant::now();
        let answer = model.run(&request, &RunContext::detached()).unwrap();
        println!("run {round}: {:.2?}", started.elapsed());
        let Payload::Inline { data, .. } = &answer[0].payload else {
            panic!("not inline");
        };
        std::fs::write(&out, data).unwrap();
    }
}

/// A load with nothing missing, no error and only recomputed buffers left over is complete; one
/// that misses a weight, had an error, or left a tensor this network does not have is not.
#[test]
fn a_load_that_is_not_whole_is_refused() {
    use burn_store::{ApplyError, ApplyResult};
    let clean = ApplyResult {
        applied: vec!["backbone.norm0.weight".into()],
        skipped: Vec::new(),
        missing: Vec::new(),
        unused: vec![
            "backbone.layers.0.blocks.0.attn.relative_position_index".into(),
            "context1.branch0.conv0.bn.num_batches_tracked".into(),
        ],
        errors: Vec::new(),
    };
    assert!(weights::complete(&clean));
    let mut missing = clean.clone();
    missing
        .missing
        .push(("decoder.conv1.conv.weight".into(), "Struct:Conv2d".into()));
    assert!(!weights::complete(&missing));
    let mut failed = clean.clone();
    failed.errors.push(ApplyError::LoadError {
        path: "backbone.norm0.weight".into(),
        message: "unreadable".into(),
    });
    assert!(!weights::complete(&failed));
    let mut foreign = clean.clone();
    foreign.unused.push("head.classifier.weight".into());
    assert!(!weights::complete(&foreign));
}
