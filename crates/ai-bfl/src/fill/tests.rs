use super::*;
use craft_ai_core::CapabilityId;
use craft_ai_core::payload::base64_decode;

fn png_of(
    color: ColorType,
    width: u32,
    height: u32,
    pixel: impl Fn(u32, u32) -> Vec<u8>,
) -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..height {
        for x in 0..width {
            data.extend(pixel(x, y));
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    out
}

fn decode(png: &[u8]) -> (ColorType, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    buf.truncate(frame.buffer_size());
    (frame.color_type, buf)
}

/// Grey mask, white on the left half (the area to fill).
fn left_half() -> Vec<u8> {
    png_of(ColorType::Grayscale, 4, 2, |x, _| {
        vec![if x < 2 { 255 } else { 0 }]
    })
}

fn source(width: u32, height: u32) -> Vec<u8> {
    png_of(ColorType::Rgb, width, height, |_, _| vec![10, 20, 30])
}

fn request(source: Vec<u8>, mask: Vec<u8>) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source).unwrap(),
        )
        .with_input(InputRole::Mask, Payload::inline("image/png", mask).unwrap())
        .with_input(InputRole::Prompt, Payload::text("a red door"))
}

#[test]
fn a_white_mask_is_sent_white_where_to_paint() {
    let (out, w, h) = bfl_mask(&left_half()).unwrap();
    assert_eq!((w, h), (4, 2));
    assert_eq!(
        decode(&out),
        (ColorType::Grayscale, vec![255, 255, 0, 0, 255, 255, 0, 0])
    );
}

#[test]
fn colour_and_grey_levels_become_binary() {
    let mask = png_of(ColorType::Rgb, 3, 1, |x, _| match x {
        0 => vec![250, 250, 250],
        1 => vec![140, 140, 140],
        _ => vec![100, 100, 100],
    });
    let (out, ..) = bfl_mask(&mask).unwrap();
    assert_eq!(decode(&out).1, vec![255, 255, 0]);
}

#[test]
fn a_transparent_mask_paints_where_it_is_clear() {
    // OpenAI's way: opaque (any colour) is kept, transparent is painted.
    let mask = png_of(ColorType::Rgba, 2, 1, |x, _| {
        if x == 0 {
            vec![255, 255, 255, 255]
        } else {
            vec![0, 0, 0, 0]
        }
    });
    let (out, ..) = bfl_mask(&mask).unwrap();
    assert_eq!(decode(&out).1, vec![0, 255]);
    // A fully opaque alpha channel says nothing, so the colour is read.
    let opaque = png_of(ColorType::GrayscaleAlpha, 2, 1, |x, _| {
        vec![if x == 0 { 255 } else { 0 }, 255]
    });
    assert_eq!(decode(&bfl_mask(&opaque).unwrap().0).1, vec![255, 0]);
}

#[test]
fn bad_masks_are_refused() {
    let empty = png_of(ColorType::Grayscale, 2, 2, |_, _| vec![0]);
    assert!(bfl_mask(&empty).unwrap_err().message.contains("nothing"));
    assert_eq!(
        bfl_mask(&[0xff, 0xd8, 0xff, 0xe0]).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
    let mut broken = left_half();
    broken.truncate(30);
    assert_eq!(
        bfl_mask(&broken).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
    let huge = png_of(ColorType::Grayscale, MAX_SIDE + 1, 1, |_, _| vec![255]);
    assert!(bfl_mask(&huge).unwrap_err().message.contains("8192"));
}

#[test]
fn body_has_prompt_pictures_png_output_and_checked_options() {
    let r = request(source(4, 2), left_half())
        .with_param("steps", json!(30))
        .with_param("guidance", json!(42.5))
        .with_param("seed", json!(7))
        .with_param("safety_tolerance", json!(2))
        .with_param("prompt_upsampling", json!(false));
    let v = body(&r).unwrap();
    assert_eq!(v["prompt"], "a red door");
    assert_eq!(v["output_format"], "png");
    assert_eq!(v["steps"], 30);
    assert_eq!(v["seed"], 7);
    assert_eq!(
        base64_decode(v["image"].as_str().unwrap()).unwrap(),
        source(4, 2)
    );
    let mask = base64_decode(v["mask"].as_str().unwrap()).unwrap();
    assert_eq!(decode(&mask).0, ColorType::Grayscale);
    let jpeg = request(source(4, 2), left_half()).with_param("output_format", json!("jpeg"));
    assert_eq!(body(&jpeg).unwrap()["output_format"], "jpeg");
}

#[test]
fn bad_requests_are_refused_before_sending() {
    let cases = [
        request(source(4, 2), left_half()).with_param("steps", json!(51)),
        request(source(4, 2), left_half()).with_param("guidance", json!(1)),
        request(source(4, 2), left_half()).with_param("safety_tolerance", json!(7)),
        request(source(4, 2), left_half()).with_param("output_format", json!("gif")),
        request(source(5, 2), left_half()),
        request(b"not a picture".to_vec(), left_half()),
    ];
    for r in cases {
        assert_eq!(
            body(&r).unwrap_err().kind,
            ErrorKind::InvalidRequest,
            "{:?}",
            r.params
        );
    }
    let unknown = request(source(4, 2), left_half()).with_param("size", json!("1024x1024"));
    assert_eq!(body(&unknown).unwrap_err().kind, ErrorKind::Unsupported);
    let blank = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source(4, 2)).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", left_half()).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("  "));
    assert_eq!(body(&blank).unwrap_err().kind, ErrorKind::InvalidRequest);
    let handle = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::handle("file:///a.png", "image/png", None).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", left_half()).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("door"));
    assert_eq!(body(&handle).unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn a_jpeg_source_is_sent_as_is() {
    let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
    let v = body(&request(jpeg.clone(), left_half())).unwrap();
    assert_eq!(base64_decode(v["image"].as_str().unwrap()).unwrap(), jpeg);
}

#[test]
fn only_bfl_delivery_hosts_are_downloaded() {
    assert!(delivery_allowed(
        "https://delivery-eu1.bfl.ai/results/a/sample.png?se=1&sig=x",
        false
    ));
    assert!(delivery_allowed("https://delivery.us.bfl.ai/x.png", false));
    for bad in [
        "http://delivery.bfl.ai/x.png",
        "https://delivery.bfl.ai.evil.example/x.png",
        "https://evilbfl.ai/x.png",
        "https://user@delivery.bfl.ai/x.png",
        "https://delivery.bfl.ai:8443/x.png",
        "https://127.0.0.1/x.png",
        "ftp://delivery.bfl.ai/x.png",
        "https://delivery.bfl.ai/x y.png",
    ] {
        assert!(!delivery_allowed(bad, false), "{bad}");
    }
    assert!(delivery_allowed("http://127.0.0.1:9/x.png", true));
    assert!(!delivery_allowed("http://127.0.0.1:9/x.png", false));
}
