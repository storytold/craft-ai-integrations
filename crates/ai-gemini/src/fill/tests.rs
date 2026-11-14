#![allow(clippy::unwrap_used)]
use super::*;
use craft_ai_core::CapabilityId;
use png::{BitDepth, ColorType};
use std::io::Cursor;

fn png_of(
    width: u32,
    height: u32,
    color: ColorType,
    pixel: impl Fn(u32, u32) -> Vec<u8>,
) -> Vec<u8> {
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    out
}

fn decoded(png: &[u8]) -> (ColorType, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    buf.truncate(frame.buffer_size());
    (frame.color_type, buf)
}

fn request(source: Vec<u8>, mask: Vec<u8>, prompt: &str) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source).unwrap(),
        )
        .with_input(InputRole::Mask, Payload::inline("image/png", mask).unwrap())
        .with_input(InputRole::Prompt, Payload::text(prompt))
}

fn gray(w: u32, h: u32, v: u8) -> Vec<u8> {
    png_of(w, h, ColorType::Grayscale, |_, _| vec![v])
}

#[test]
fn a_transparent_mask_becomes_opaque_white_where_to_fill() {
    // PhotoCraft's mask: transparent and white where the fill goes, opaque black elsewhere.
    let mask = png_of(4, 1, ColorType::Rgba, |x, _| {
        if x < 2 {
            vec![255, 255, 255, 0]
        } else {
            vec![0, 0, 0, 255]
        }
    });
    let (shown, w, h) = shown_mask(&mask).unwrap();
    assert_eq!((w, h), (4, 1));
    assert_eq!(
        decoded(&shown),
        (ColorType::Grayscale, vec![255, 255, 0, 0])
    );
    // Without transparency, brightness decides.
    let (shown, ..) = shown_mask(&png_of(2, 1, ColorType::Grayscale, |x, _| {
        vec![if x == 0 { 200 } else { 40 }]
    }))
    .unwrap();
    assert_eq!(decoded(&shown).1, vec![255, 0]);
}

#[test]
fn a_mask_that_marks_nothing_or_is_not_a_png_is_refused() {
    assert!(
        shown_mask(&gray(3, 3, 0))
            .unwrap_err()
            .message
            .contains("nothing")
    );
    assert_eq!(
        shown_mask(b"\xff\xd8\xffjpeg").unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
}

#[test]
fn the_body_sends_instruction_source_and_mask_with_ratio_and_size() {
    let body = body(
        "gemini-nano-banana-2.1",
        &request(gray(1600, 900, 90), gray(1600, 900, 255), "a red balloon."),
    )
    .unwrap();
    assert_eq!(body["model"], "gemini-nano-banana-2.1");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert!(
        input[0]["text"]
            .as_str()
            .unwrap()
            .contains("The requested change is: \"a red balloon\".")
    );
    assert_eq!(input[1]["data"], base64_encode(&gray(1600, 900, 90)));
    assert_eq!(input[2]["mime_type"], "image/png");
    assert_eq!(body["response_format"]["aspect_ratio"], "16:9");
    assert_eq!(body["response_format"]["image_size"], "2K");
    assert_eq!(body["response_format"]["mime_type"], "image/jpeg");
}

#[test]
fn size_follows_the_picture_unless_asked_and_lite_stays_at_1k() {
    assert_eq!(size_token("gemini-3-pro-image", None, 1024, 300), "1K");
    assert_eq!(size_token("gemini-3-pro-image", None, 3000, 300), "4K");
    assert_eq!(size_token("gemini-3-pro-image", Some("4K"), 10, 10), "4K");
    assert_eq!(
        size_token("gemini-3.1-flash-lite-image", Some("4K"), 4000, 4000),
        "1K"
    );
    let asked = request(gray(8, 8, 1), gray(8, 8, 255), "")
        .with_param("image_size", serde_json::json!("4K"));
    assert_eq!(
        body("gemini-3-pro-image", &asked).unwrap()["response_format"]["image_size"],
        "4K"
    );
    let bad = request(gray(8, 8, 1), gray(8, 8, 255), "")
        .with_param("image_size", serde_json::json!("8K"));
    assert_eq!(
        body("gemini-3-pro-image", &bad).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
    let other =
        request(gray(8, 8, 1), gray(8, 8, 255), "").with_param("seed", serde_json::json!(1));
    assert_eq!(
        body("gemini-3-pro-image", &other).unwrap_err().kind,
        ErrorKind::Unsupported
    );
}

#[test]
fn ratios_snap_to_the_nearest_supported_one() {
    assert_eq!(nearest_ratio(1920, 1080), (16, 9));
    assert_eq!(nearest_ratio(1000, 1000), (1, 1));
    assert_eq!(nearest_ratio(600, 800), (3, 4));
    assert_eq!(nearest_ratio(3000, 1000), (21, 9));
    assert_eq!(nearest_ratio(0, 5), (1, 1));
}

#[test]
fn a_mask_of_another_size_is_refused() {
    let e = body(
        "gemini-3-pro-image",
        &request(gray(8, 8, 1), gray(9, 8, 255), "x"),
    )
    .unwrap_err();
    assert!(e.message.contains("same size"));
}

#[test]
fn an_empty_prompt_asks_for_removal() {
    let empty = fill_instruction("  ");
    assert!(
        empty.contains("had never existed") && empty.contains("Do not add a new"),
        "{empty}"
    );
    assert!(!empty.contains("requested change"));
    let boat = fill_instruction("  a boat. ");
    assert!(
        boat.contains("The requested change is: \"a boat\".")
            && !boat.contains("had never existed")
    );
    assert!(boat.contains("white marks the only area you may change"));
}

#[test]
fn only_gemini_image_models_are_allowed() {
    for ok in [
        "gemini-nano-banana-2.1",
        "gemini-3-pro-image",
        "gemini-4-flash-image-preview",
    ] {
        assert!(model_allowed(ok), "{ok}");
    }
    for bad in [
        "gemini-2.5-pro",
        "gpt-image-1",
        "gemini-image/../x",
        "Gemini-3-pro-image",
    ] {
        assert!(!model_allowed(bad), "{bad}");
    }
}

#[test]
fn answers_are_read_from_output_image_steps_or_text() {
    let png = gray(1, 1, 7);
    let b64 = base64_encode(&png);
    let top = serde_json::json!({"output_image": {"mime_type": "image/png", "data": b64}});
    assert!(matches!(read_answer(&top).unwrap(), Answer::Image("image/png", ref b) if *b == png));
    let steps = serde_json::json!({"steps": [{"type": "model_output", "content": [{"type": "text", "text": "Here"}, {"type": "image", "data": b64}]}]});
    assert!(matches!(read_answer(&steps).unwrap(), Answer::Image(..)));
    let refused = serde_json::json!({"output_text": " I can't help with that. "});
    assert!(
        matches!(read_answer(&refused).unwrap(), Answer::Text(ref t) if t == "I can't help with that.")
    );
    let said = serde_json::json!({"steps": [{"content": [{"type": "text", "text": "No."}]}]});
    assert!(matches!(read_answer(&said).unwrap(), Answer::Text(ref t) if t == "No."));
    assert!(
        matches!(read_answer(&serde_json::json!({})).unwrap(), Answer::Text(ref t) if t.is_empty())
    );
    let junk = serde_json::json!({"output_image": {"data": "%%%"}});
    assert_eq!(read_answer(&junk).err().unwrap().kind, ErrorKind::Provider);
    let not_a_picture = serde_json::json!({"output_image": {"data": base64_encode(b"hello")}});
    assert_eq!(
        read_answer(&not_a_picture).err().unwrap().kind,
        ErrorKind::Provider
    );
}

#[test]
fn clip_keeps_short_text_and_cuts_long_text_on_a_character() {
    assert_eq!(clip("short", 10), "short");
    assert_eq!(clip("ééééé", 2), "éé…");
}
