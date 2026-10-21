use super::*;

/// A PNG of `width` by `height` in the given colour type, every sample `value`.
fn png_of(color: ColorType, width: u32, height: u32, value: u8) -> Vec<u8> {
    let channels = match color {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        _ => 4,
    };
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let samples = vec![value; (width * height) as usize * channels];
    writer.write_image_data(&samples).unwrap();
    writer.finish().unwrap();
    out
}

/// A grayscale PNG holding exactly `samples`, `width` wide.
fn gray_png(width: u32, samples: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, samples.len() as u32 / width);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(samples).unwrap();
    writer.finish().unwrap();
    out
}

/// The start of a JPEG: an application segment, then a baseline frame header for the size.
fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
    let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F'];
    b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
    b
}

/// An image of the given media type holding `data`.
fn image(media_type: &str, data: Vec<u8>) -> Image {
    Image {
        media_type: media_type.to_owned(),
        data,
    }
}

/// A mask size, with or without a possible turn.
fn size(width: u32, height: u32, turnable: bool) -> Size {
    Size {
        width,
        height,
        turnable,
    }
}

/// A job event carrying `png` as an inline PNG output.
fn event_of(png: Vec<u8>) -> Event {
    Event::Output(Output {
        payload: Payload::inline("image/png", png).unwrap(),
        label: None,
    })
}

/// The bytes of the first output, which has to be inline.
fn mask_of(events: &[Event]) -> Vec<u8> {
    let Some(Event::Output(Output {
        payload: Payload::Inline { data, .. },
        ..
    })) = events.first()
    else {
        panic!("an inline mask is expected first");
    };
    data.clone()
}

/// The `Found` result the events carry.
fn found_of(events: &[Event]) -> Found {
    let outputs: Vec<Output> = events
        .iter()
        .filter_map(|e| match e {
            Event::Output(o) => Some(o.clone()),
            _ => None,
        })
        .collect();
    segment::read_found(&outputs).unwrap()
}

/// The picture's size comes from its header, whichever format, and a JPEG may come back turned.
#[test]
fn the_size_comes_from_the_header_of_a_png_or_a_jpeg() {
    let png = image("image/png", png_of(ColorType::Rgb, 7, 3, 9));
    assert_eq!(size_of(&png).unwrap(), size(7, 3, false));
    let jpeg = image("image/jpeg", jpeg_header(640, 480));
    assert_eq!(size_of(&jpeg).unwrap(), size(640, 480, true));
}

/// Anything whose size cannot be read, or is empty or over 100 megapixels, is refused.
#[test]
fn a_picture_with_no_readable_or_a_huge_size_is_refused() {
    for bad in [
        image("image/png", b"\x89PNG\r\n\x1a\nxx".to_vec()),
        image("image/jpeg", vec![0xFF, 0xD8, 0xFF]),
        image("image/jpeg", jpeg_header(0, 10)),
        image("image/jpeg", jpeg_header(60_000, 60_000)),
        image("image/webp", b"RIFF".to_vec()),
    ] {
        assert!(size_of(&bad).is_err());
    }
}

/// A JPEG's segments before the frame header are skipped by their lengths, and a table marker
/// (C4) is not taken for the frame.
#[test]
fn jpeg_segments_before_the_frame_are_skipped() {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00];
    data.extend_from_slice(&jpeg_header(30, 20)[2..]);
    assert_eq!(jpeg_size(&data), Some((30, 20)));
    assert_eq!(jpeg_size(&[0xFF, 0xD8, 0x00, 0x00]), None);
}

/// The same bytes give the same key; any other bytes give another.
#[test]
fn the_cache_key_follows_the_bytes() {
    let a = image("image/png", png_of(ColorType::Rgb, 2, 2, 1));
    let same = image("image/png", a.data.clone());
    let other = image("image/png", png_of(ColorType::Rgb, 2, 2, 2));
    assert_eq!(cache_key(&a), cache_key(&same));
    assert_ne!(cache_key(&a), cache_key(&other));
}

/// Any colour type is reduced to one channel, white kept, and the answer is a grayscale PNG at the
/// picture's size followed by a result saying it matched.
#[test]
fn a_selected_mask_is_one_channel_and_found() {
    for color in [
        ColorType::Rgb,
        ColorType::Rgba,
        ColorType::Grayscale,
        ColorType::GrayscaleAlpha,
    ] {
        let events = answer(event_of(png_of(color, 6, 4, 255)), &size(6, 4, false)).unwrap();
        assert_eq!(events.len(), 2);
        let decoded = png::Decoder::new(Cursor::new(mask_of(&events)))
            .read_info()
            .unwrap();
        assert_eq!(decoded.info().color_type, ColorType::Grayscale);
        assert_eq!((decoded.info().width, decoded.info().height), (6, 4));
        assert!(found_of(&events).matched, "{color:?}");
    }
}

/// A mask with nothing above half is an empty mask and `matched: false`, not a failed job.
#[test]
fn a_mask_with_nothing_selected_is_no_match_not_a_failure() {
    let events = answer(
        event_of(png_of(ColorType::Rgb, 5, 5, 100)),
        &size(5, 5, false),
    )
    .unwrap();
    assert!(!found_of(&events).matched);
    let mut reader = png::Decoder::new(Cursor::new(mask_of(&events)))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buffer).unwrap();
    assert!(buffer.iter().all(|v| *v == 0));
}

/// A single selected pixel is a match; the line is half.
#[test]
fn half_is_the_line_between_selected_and_not() {
    assert!(
        finish(&gray_png(2, &[127, 128]), &size(2, 1, false))
            .unwrap()
            .1
    );
    assert!(
        !finish(&gray_png(2, &[127, 127]), &size(2, 1, false))
            .unwrap()
            .1
    );
}

/// A mask of another size is refused; a turned one is accepted only for a JPEG.
#[test]
fn a_mask_of_the_wrong_size_is_refused_and_a_turn_is_allowed_only_for_a_jpeg() {
    let mask = png_of(ColorType::Rgb, 4, 8, 255);
    let e = finish(&mask, &size(8, 4, false)).unwrap_err();
    assert!(e.message.contains("not the size"), "{}", e.message);
    assert!(finish(&mask, &size(8, 4, true)).is_ok());
    assert!(finish(&mask, &size(9, 4, true)).is_err());
    assert!(finish(&mask, &size(4, 9, true)).is_err());
}

/// What is not a PNG mask is refused, and a JPEG answer is said to be the wrong kind.
#[test]
fn something_that_is_not_a_png_mask_is_refused() {
    let jpeg = Event::Output(Output {
        payload: Payload::inline("image/jpeg", vec![0xFF, 0xD8, 0xFF, 0xE0]).unwrap(),
        label: None,
    });
    assert_eq!(
        answer(jpeg, &size(2, 2, false)).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    for event in [
        Event::Notice {
            message: "x".into(),
        },
        event_of(b"\x89PNG not really".to_vec()),
    ] {
        assert_eq!(
            answer(event, &size(2, 2, false)).unwrap_err().kind,
            ErrorKind::Provider
        );
    }
}

/// The first channel is the mask: a picture with a lit alpha but dark colour is not selected.
#[test]
fn the_first_channel_is_the_mask_not_the_alpha() {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 1, 1);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 0, 0, 255])
        .unwrap();
    assert!(!finish(&out, &size(1, 1, false)).unwrap().1);
}
