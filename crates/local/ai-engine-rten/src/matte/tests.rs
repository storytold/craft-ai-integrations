use super::*;
use craft_ai_core::{ErrorKind, Request};

/// Models that are never asked: a run that reaches them fails the test.
struct Unused;

impl pipeline::Models for Unused {
    fn subject(&mut self, _: Vec<f32>) -> Result<Vec<f32>, AiError> {
        panic!("the subject model was run")
    }
    fn matte(&mut self, _: bool, _: Vec<f32>, _: usize, _: usize) -> Result<Vec<f32>, AiError> {
        panic!("a matting model was run")
    }
}

#[test]
fn a_request_without_a_picture_is_refused_before_any_model_runs() {
    let mut model = Loaded::new(Unused);
    let r = Request::new(CapabilityId::parse(CAPABILITY).unwrap());
    let e = model.run(&r, &RunContext::detached()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
}

#[test]
fn an_answer_over_the_inline_limit_is_refused_in_words() {
    let limits = Limits {
        max_inline_bytes: 100,
        ..Limits::default()
    };
    assert!(answer(vec![0; 100], &limits).is_ok());
    let e = answer(vec![0; 101], &limits).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert!(e.message.contains("101 bytes"), "{}", e.message);
}

fn missing_files(e: &AiError) -> bool {
    e.message.contains("together in one folder")
}

#[test]
fn loading_without_the_three_files_says_what_is_missing() {
    // Each file missing in turn, the others present (and not models).
    for missing in [BEN2_FILE, MATTE_SMALL_FILE, MATTE_BASE_FILE] {
        let dir = tempfile::tempdir().unwrap();
        for file in [BEN2_FILE, MATTE_SMALL_FILE, MATTE_BASE_FILE] {
            if file != missing {
                std::fs::write(dir.path().join(file), b"x").unwrap();
            }
        }
        let Err(e) = MatteRunner.load(&dir.path().join(BEN2_FILE)) else {
            panic!("loaded without {missing}")
        };
        assert!(missing_files(&e), "without {missing}: {}", e.message);
    }
    let dir = tempfile::tempdir().unwrap();
    for file in [BEN2_FILE, MATTE_SMALL_FILE, MATTE_BASE_FILE] {
        std::fs::write(dir.path().join(file), b"x").unwrap();
    }
    // The matting models beside a weights file other than ben2.onnx are not enough.
    let Err(e) = MatteRunner.load(&dir.path().join(MATTE_BASE_FILE)) else {
        panic!("loaded from the wrong weights file")
    };
    assert!(missing_files(&e), "{}", e.message);
    // All three present but not models: refused, not a panic.
    let Err(e) = MatteRunner.load(&dir.path().join(BEN2_FILE)) else {
        panic!("loaded files that are not models")
    };
    assert!(!missing_files(&e), "{}", e.message);
}

#[test]
fn a_file_over_512_mb_is_refused_before_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    // A sparse file: its length is over the bound but nothing is written.
    let big = std::fs::File::create(dir.path().join(MATTE_BASE_FILE)).unwrap();
    big.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(big);
    let Err(e) = Rten::read(dir.path(), MATTE_BASE_FILE) else {
        panic!("read a file over the bound")
    };
    assert_eq!(e.kind, ErrorKind::Unsupported, "{}", e.message);
    assert!(
        e.message.contains("not the BEN2 + ViTMatte models"),
        "{}",
        e.message
    );
}

#[test]
fn the_runner_offers_image_matte_on_rten() {
    let info = MatteRunner.model();
    assert_eq!(MatteRunner.engine(), "rten");
    assert_eq!(info.id.as_str(), MODEL);
    assert!(
        info.capabilities
            .iter()
            .any(|c| c.capability.as_str() == CAPABILITY)
    );
}
