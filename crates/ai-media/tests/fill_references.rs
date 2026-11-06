//! The shared reference-picture check and the instruction that names references.
use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_media::mask::{
    Bounds, ReferenceRule, fill_instruction, fill_instruction_with_references, references,
};

const BOUNDS: Bounds = Bounds {
    service: "Test fill",
    max_side: 64,
    max_pixels: 64 * 64,
};

const RULE: ReferenceRule = ReferenceRule {
    max_count: 2,
    max_bytes: 4096,
    png_only: false,
};

/// A `w` x `h` RGB PNG of one colour.
fn png_rgb(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&rgb.repeat((w * h) as usize))
        .unwrap();
    writer.finish().unwrap();
    out
}

/// A real 4 x 4 JPEG.
fn jpeg() -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]));
    let mut out = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

/// A request whose references are `refs`, with a source and a mask in front of them.
fn request(refs: Vec<Payload>) -> Request {
    let mut r = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png_rgb(4, 4, [1, 1, 1])).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png_rgb(4, 4, [255, 255, 255])).unwrap(),
        );
    for p in refs {
        r = r.with_input(InputRole::Reference, p);
    }
    r
}

/// An inline payload of `data` labelled PNG (the label is not trusted).
fn inline(data: Vec<u8>) -> Payload {
    Payload::inline("image/png", data).unwrap()
}

/// References come back in request order, each with its format read from the bytes; none is fine.
#[test]
fn references_keep_request_order_and_real_format() {
    let first = png_rgb(4, 4, [200, 0, 0]);
    let second = jpeg();
    let req = request(vec![inline(first.clone()), inline(second.clone())]);
    let got = references(&req, BOUNDS, RULE).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].data, got[0].media), (first.as_slice(), "image/png"));
    assert_eq!(
        (got[1].data, got[1].media),
        (second.as_slice(), "image/jpeg")
    );
    assert!(
        references(&request(vec![]), BOUNDS, RULE)
            .unwrap()
            .is_empty()
    );
}

/// Every rule refuses with its own error before anything could be sent.
#[test]
fn each_rule_refuses() {
    let ok = || inline(png_rgb(4, 4, [5, 5, 5]));
    let kind = |refs: Vec<Payload>, rule: ReferenceRule| {
        references(&request(refs), BOUNDS, rule).unwrap_err().kind
    };
    assert_eq!(
        kind(vec![ok(), ok(), ok()], RULE),
        ErrorKind::Unsupported,
        "count"
    );
    let none = ReferenceRule {
        max_count: 0,
        ..RULE
    };
    let err = references(&request(vec![ok()]), BOUNDS, none).unwrap_err();
    assert!(
        err.message.contains("takes no reference pictures"),
        "{}",
        err.message
    );
    let link = Payload::handle("https://example.com/r.png", "image/png", None).unwrap();
    assert_eq!(kind(vec![link], RULE), ErrorKind::Unsupported, "a link");
    let big = ReferenceRule {
        max_bytes: 10,
        ..RULE
    };
    assert_eq!(kind(vec![ok()], big), ErrorKind::InvalidRequest, "bytes");
    assert_eq!(
        kind(vec![inline(b"plain text".to_vec())], RULE),
        ErrorKind::InvalidRequest
    );
    let png_only = ReferenceRule {
        png_only: true,
        ..RULE
    };
    assert_eq!(
        kind(vec![inline(jpeg())], png_only),
        ErrorKind::InvalidRequest,
        "png only"
    );
    assert_eq!(
        kind(vec![inline(png_rgb(65, 1, [0, 0, 0]))], RULE),
        ErrorKind::InvalidRequest,
        "too wide"
    );
    let mut cut = png_rgb(8, 8, [7, 7, 7]);
    cut.truncate(cut.len() - 20);
    assert_eq!(
        kind(vec![inline(cut)], RULE),
        ErrorKind::InvalidRequest,
        "broken"
    );
}

/// No references leaves the fill instruction as it was; one or more are named by image number.
#[test]
fn the_instruction_names_references_by_image_number() {
    assert_eq!(
        fill_instruction_with_references("a cat", 0),
        fill_instruction("a cat")
    );
    let one = fill_instruction_with_references("a cat", 1);
    assert!(one.starts_with(&fill_instruction("a cat")));
    assert!(
        one.contains("The third image is a reference picture"),
        "{one}"
    );
    assert!(one.contains("do not change the black area"), "{one}");
    let three = fill_instruction_with_references("", 3);
    assert!(
        three.contains("Images 3 to 5 are reference pictures"),
        "{three}"
    );
}
