use std::sync::Arc;

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Limits, Payload, Request};
use craft_ai_engine::testing::check_runner;
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use png::{BitDepth, ColorType};

use super::{
    CAPABILITY, LAMA_FILE, LamaRunner, Loaded, MAX_FILE_BYTES, MODEL, Rten, answer, codec, pipeline,
};
use crate::picture::decode_picture;

/// Model that fails the test if reached (proves input checks fire before inference).
struct NeverCalled;

impl pipeline::Inpainter for NeverCalled {
    fn inpaint(&mut self, _: Vec<f32>, _: Vec<f32>) -> Result<Vec<f32>, craft_ai_core::AiError> {
        panic!("the LaMa model was called on an invalid request")
    }
}

/// Deterministic test model: fills masked pixels with solid green `(20, 200, 60)` and writes zero
/// outside the mask so unit tests prove the compositor never overwrites pixels outside the rim.
struct FillGreen;

impl pipeline::Inpainter for FillGreen {
    fn inpaint(
        &mut self,
        image: Vec<f32>,
        mask: Vec<f32>,
    ) -> Result<Vec<f32>, craft_ai_core::AiError> {
        let plane = pipeline::SIDE * pipeline::SIDE;
        assert_eq!(image.len(), 3 * plane);
        assert_eq!(mask.len(), plane);
        // Verify normalized input bounds.
        assert!(image.iter().all(|&v| (0.0..=1.0).contains(&v)));
        assert!(mask.iter().all(|&m| m == 0.0 || m == 1.0));
        assert!(mask.contains(&1.0), "the dilated mask must mark pixels");

        let fill = [20.0f32, 200.0, 60.0];
        let mut out = vec![0.0f32; 3 * plane];
        for i in 0..plane {
            if mask[i] > 0.5 {
                for c in 0..3 {
                    out[c * plane + i] = fill[c];
                }
            }
        }
        Ok(out)
    }
}

fn png_of(color: ColorType, w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(color);
    enc.set_depth(BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(data).unwrap();
    writer.finish().unwrap();
    out
}

fn solid_rgb_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..(w * h) {
        pixels.extend_from_slice(&rgb);
    }
    png_of(ColorType::Rgb, w, h, &pixels)
}

fn square_mask_png(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<u8> {
    let mut gray = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            gray.push(if x >= x0 && x < x1 && y >= y0 && y < y1 {
                255
            } else {
                0
            });
        }
    }
    png_of(ColorType::Grayscale, w, h, &gray)
}

fn inpaint_request(source: Vec<u8>, mask: Vec<u8>) -> Request {
    Request::new(CapabilityId::parse(CAPABILITY).unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source).unwrap(),
        )
        .with_input(InputRole::Mask, Payload::inline("image/png", mask).unwrap())
}

#[test]
fn the_runner_offers_prompt_free_image_inpaint_on_rten() {
    let info = LamaRunner.model();
    assert_eq!(LamaRunner.engine(), "rten");
    assert_eq!(info.id.as_str(), MODEL);
    let cap = CapabilityId::parse(CAPABILITY).unwrap();
    let spec = info.spec_for(&cap).expect("offers image.inpaint");
    assert!(spec.requires.contains(&InputRole::Source));
    assert!(spec.requires.contains(&InputRole::Mask));
    assert!(
        !spec.accepts.contains_key(&InputRole::Prompt),
        "LaMa is prompt-blind and must not be offered for prompt-guided Generative fill"
    );
}

#[test]
fn missing_inputs_options_and_links_are_refused_before_any_model_runs() {
    let cap = CapabilityId::parse(CAPABILITY).unwrap();
    let src = solid_rgb_png(32, 32, [220, 40, 40]);
    let msk = square_mask_png(32, 32, 8, 8, 24, 24);

    // Missing source.
    let mut model = Loaded::new(NeverCalled);
    let r = Request::new(cap.clone()).with_input(
        InputRole::Mask,
        Payload::inline("image/png", msk.clone()).unwrap(),
    );
    assert_eq!(
        model.run(&r, &RunContext::detached()).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );

    // Missing mask.
    let r = Request::new(cap.clone()).with_input(
        InputRole::Source,
        Payload::inline("image/png", src.clone()).unwrap(),
    );
    assert_eq!(
        model.run(&r, &RunContext::detached()).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );

    // Unknown request params refused.
    let mut with_opt = inpaint_request(src.clone(), msk.clone());
    with_opt
        .params
        .insert("strength".into(), serde_json::json!(0.5));
    assert_eq!(
        model
            .run(&with_opt, &RunContext::detached())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );

    // Link instead of inline bytes refused.
    let link_req = Request::new(cap)
        .with_input(
            InputRole::Source,
            Payload::handle("https://example.com/a.png", "image/png", None).unwrap(),
        )
        .with_input(InputRole::Mask, Payload::inline("image/png", msk).unwrap());
    assert_eq!(
        model
            .run(&link_req, &RunContext::detached())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
}

#[test]
fn mask_size_mismatch_blank_mask_and_alpha_hole_masks_behave_as_documented() {
    let src = solid_rgb_png(40, 30, [200, 50, 50]);

    // Wrong mask dimensions refused before the model runs.
    let wrong_size = square_mask_png(30, 40, 5, 5, 15, 15);
    let mut never = Loaded::new(NeverCalled);
    let err = never
        .run(
            &inpaint_request(src.clone(), wrong_size),
            &RunContext::detached(),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    assert!(err.message.contains("same size"), "{}", err.message);

    // Blank mask (nothing marked) refused directly by decode_mask and by the runner.
    let blank = square_mask_png(40, 30, 0, 0, 0, 0);
    assert!(codec::decode_mask(&blank, 40, 30).is_err());
    let err = never
        .run(
            &inpaint_request(src.clone(), blank),
            &RunContext::detached(),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    assert!(err.message.contains("marks nothing"), "{}", err.message);

    // RGBA mask with transparent hole in center (`alpha == 0` = remove, `alpha == 255` = keep).
    let mut rgba = Vec::with_capacity(40 * 30 * 4);
    for y in 0..30 {
        for x in 0..40 {
            let hole = (12..28).contains(&x) && (8..22).contains(&y);
            rgba.extend_from_slice(&[0, 0, 0, if hole { 0 } else { 255 }]);
        }
    }
    let alpha_mask = png_of(ColorType::Rgba, 40, 30, &rgba);
    let decoded = codec::decode_mask(&alpha_mask, 40, 30).unwrap();
    assert_eq!(decoded[15 * 40 + 20], 255);
    assert_eq!(decoded[0], 0);
}

#[test]
fn small_and_large_pictures_replace_the_hole_and_leave_distant_pixels_untouched() {
    // 1. Small picture (64 x 48, smaller than 512 -> symmetric reflection padding path).
    let mut model = Loaded::new(FillGreen);
    let src_small = solid_rgb_png(64, 48, [220, 30, 40]);
    let msk_small = square_mask_png(64, 48, 24, 16, 40, 32);
    let out = model
        .run(
            &inpaint_request(src_small, msk_small),
            &RunContext::detached(),
        )
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("expected inline PNG")
    };
    let small_rgb = decode_picture(data).unwrap();
    assert_eq!((small_rgb.width, small_rgb.height), (64, 48));
    let center = (24 * 64 + 32) * 3;
    assert_eq!(&small_rgb.bytes[center..center + 3], &[20, 200, 60]);
    // Top-left corner is far outside the dilated rim and must stay bit-for-bit identical.
    assert_eq!(&small_rgb.bytes[0..3], &[220, 30, 40]);

    // 2. Large picture (960 x 720, crop > 512 when removing a 420 px region -> resample path).
    let src_large = solid_rgb_png(960, 720, [40, 60, 210]);
    let msk_large = square_mask_png(960, 720, 260, 150, 680, 570);
    let out = model
        .run(
            &inpaint_request(src_large, msk_large),
            &RunContext::detached(),
        )
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("expected inline PNG")
    };
    let large_rgb = decode_picture(data).unwrap();
    assert_eq!((large_rgb.width, large_rgb.height), (960, 720));
    let mid = (360 * 960 + 470) * 3;
    assert_eq!(&large_rgb.bytes[mid..mid + 3], &[20, 200, 60]);
    assert_eq!(&large_rgb.bytes[0..3], &[40, 60, 210]);
}

#[test]
fn wrong_model_answer_size_and_over_limit_payloads_are_refused() {
    struct ShortAnswer;
    impl pipeline::Inpainter for ShortAnswer {
        fn inpaint(
            &mut self,
            _: Vec<f32>,
            _: Vec<f32>,
        ) -> Result<Vec<f32>, craft_ai_core::AiError> {
            Ok(vec![0.0; 16])
        }
    }
    let mut model = Loaded::new(ShortAnswer);
    let req = inpaint_request(
        solid_rgb_png(32, 32, [10, 20, 30]),
        square_mask_png(32, 32, 8, 8, 16, 16),
    );
    let err = model.run(&req, &RunContext::detached()).unwrap_err();
    assert!(err.message.contains("wrong size"), "{}", err.message);

    let limits = Limits {
        max_inline_bytes: 64,
        ..Limits::default()
    };
    assert!(answer(vec![0; 64], &limits).is_ok());
    let over = answer(vec![0; 65], &limits).unwrap_err();
    assert_eq!(over.kind, ErrorKind::InvalidRequest);
    assert!(over.message.contains("65 bytes"), "{}", over.message);
}

#[test]
fn loading_checks_the_filename_size_bound_and_valid_onnx_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("other.onnx");
    std::fs::write(&wrong, b"x").unwrap();
    let Err(e) = LamaRunner.load(&wrong) else {
        panic!("loaded wrong file name")
    };
    assert!(e.message.contains("needs lama.onnx"), "{}", e.message);

    let missing = dir.path().join(LAMA_FILE);
    let Err(e) = LamaRunner.load(&missing) else {
        panic!("loaded missing file")
    };
    assert!(e.message.contains("needs lama.onnx"), "{}", e.message);

    // Sparse file over MAX_FILE_BYTES refused before reading.
    let big_path = dir.path().join(LAMA_FILE);
    let big = std::fs::File::create(&big_path).unwrap();
    big.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(big);
    let Err(e) = Rten::read(&big_path) else {
        panic!("read a file over 512 MB")
    };
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("not the LaMa model"), "{}", e.message);

    // Corrupt bytes refused cleanly without panic.
    std::fs::write(&big_path, b"not-an-onnx-file").unwrap();
    assert!(LamaRunner.load(&big_path).is_err());
}

#[test]
fn synthetic_runner_passes_the_shared_conformance_checks() {
    struct FakeLamaRunner;
    impl ModelRunner for FakeLamaRunner {
        fn engine(&self) -> &str {
            LamaRunner.engine()
        }
        fn model(&self) -> craft_ai_core::ModelInfo {
            LamaRunner.model()
        }
        fn load(
            &self,
            weights: &std::path::Path,
        ) -> Result<Box<dyn LoadedModel>, craft_ai_core::AiError> {
            std::fs::metadata(weights)
                .map_err(|_| craft_ai_core::AiError::unsupported("missing weights"))?;
            Ok(Box::new(Loaded::new(FillGreen)))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join(LAMA_FILE);
    std::fs::write(&weights, b"stub").unwrap();
    let good = inpaint_request(
        solid_rgb_png(48, 48, [180, 90, 40]),
        square_mask_png(48, 48, 16, 16, 32, 32),
    );
    let bad = Request::new(CapabilityId::parse(CAPABILITY).unwrap());
    check_runner(
        Arc::new(FakeLamaRunner),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}
