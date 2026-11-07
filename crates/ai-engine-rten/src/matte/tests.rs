use super::*;
use craft_ai_core::{ErrorKind, Request};

fn request(quality: Option<&str>) -> Request {
    let mut r = Request::new(CapabilityId::parse(CAPABILITY).unwrap());
    if let Some(q) = quality {
        r.params.insert("quality".into(), serde_json::json!(q));
    }
    r
}

#[test]
fn quality_defaults_to_max_and_refuses_others() {
    assert_eq!(read_quality(&request(None)).unwrap(), Quality::Max);
    assert_eq!(read_quality(&request(Some("high"))).unwrap(), Quality::High);
    assert_eq!(read_quality(&request(Some("max"))).unwrap(), Quality::Max);
    assert_eq!(
        read_quality(&request(Some("ultra"))).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
}

#[test]
fn loading_without_the_three_files_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ben2 = dir.path().join(BEN2_FILE);
    std::fs::write(&ben2, b"x").unwrap();
    let Err(e) = MatteRunner.load(&ben2) else {
        panic!("loaded without the matting models")
    };
    assert!(e.message.contains("vitmatte-small.onnx"), "{}", e.message);
    // The other files present but not models: refused, not a panic.
    std::fs::write(dir.path().join(MATTE_SMALL_FILE), b"x").unwrap();
    std::fs::write(dir.path().join(MATTE_BASE_FILE), b"x").unwrap();
    assert!(MatteRunner.load(&ben2).is_err());
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
