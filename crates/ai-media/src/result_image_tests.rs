//! The shared decode budget: a reservation that does not fit waits until enough is given back.
#![allow(clippy::unwrap_used)]

use super::{DECODE_BUDGET, Reservation};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// While the whole budget is held, another decode waits; once it is given back, the decode runs.
#[test]
fn a_decode_waits_while_the_budget_is_full() {
    let full = Reservation::take(DECODE_BUDGET);
    let (done, finished) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let _one = Reservation::take(1);
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(200)).is_err(),
        "a reservation over the budget must wait"
    );
    drop(full);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("giving the budget back must wake the waiting decode");
    waiter.join().unwrap();
}

/// A JPEG result's decode takes its share of the budget: it waits while the budget is full.
#[test]
fn a_jpeg_decode_reserves_from_the_budget() {
    let picture = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 40, 40]));
    let mut data = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut data, image::ImageFormat::Jpeg)
        .unwrap();
    let data = data.into_inner();
    let full = Reservation::take(DECODE_BUDGET);
    let (done, finished) = mpsc::channel();
    let decoder = thread::spawn(move || done.send(super::decoded_result(&data)).unwrap());
    assert!(
        finished.recv_timeout(Duration::from_millis(200)).is_err(),
        "the decode must wait for its share"
    );
    drop(full);
    let media = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("giving the budget back must let the decode run");
    assert_eq!(media, Some("image/jpeg"));
    decoder.join().unwrap();
}

/// A scan with stuffed zeros, restart markers and fill bytes is read through to the end marker;
/// the same scan with its end cut off never reaches one.
#[test]
fn a_scan_is_read_past_stuffed_bytes_and_restart_markers() {
    let jpeg = [
        0xFF, 0xD8, // start of image
        0xFF, 0xDA, 0x00, 0x02, // start of scan, empty header
        0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD7,
        0x78, // data, stuffing, restarts
        0xFF, 0xFF, 0xD9, // a fill byte, then the end of image
    ];
    assert!(super::reaches_eoi(&jpeg));
    assert!(!super::reaches_eoi(&jpeg[..jpeg.len() - 3]));
    assert_eq!(super::scan_end(&jpeg, 6), Some(17));
}

/// A JPEG result whose header is thousands of tiny Extended XMP fragments is refused before the
/// decoder, which re-scans every fragment after each one, sees it or the budget is taken. A
/// plausible count still decodes.
#[test]
fn a_jpeg_made_of_tiny_xmp_fragments_is_refused_before_decoding() {
    use craft_ai_core::jpeg::{EXTENDED_XMP_PREFIX, MAX_XMP_FRAGMENTS};
    let picture = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 40, 40]));
    let mut plain = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut plain, image::ImageFormat::Jpeg)
        .unwrap();
    let plain = plain.into_inner();
    let with_fragments = |n: u32| {
        let mut out = plain[..2].to_vec();
        for i in 0..n {
            let mut body = EXTENDED_XMP_PREFIX.to_vec();
            body.extend_from_slice(&[b'0'; 32]);
            body.extend_from_slice(&n.to_be_bytes());
            body.extend_from_slice(&i.to_be_bytes());
            body.push(b'x');
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
            out.extend_from_slice(&body);
        }
        out.extend_from_slice(&plain[2..]);
        out
    };
    let fine = with_fragments(MAX_XMP_FRAGMENTS as u32);
    assert_eq!(super::decoded_result(&fine), Some("image/jpeg"));
    // The end-marker walk refuses a stray byte between segments before the decoder runs.
    assert_eq!(super::decoded_result(&with_stray_byte(fine)), None);
    let hostile = with_fragments(MAX_XMP_FRAGMENTS as u32 + 1);
    assert_eq!(super::decoded_result(&hostile), None);
    assert_eq!(super::decoded_result(&with_stray_byte(hostile)), None);
}

/// The same file with one stray byte before its second Extended XMP fragment. The decoder skips
/// it and keeps reading fragments, so a guard must count past it.
fn with_stray_byte(mut jpeg: Vec<u8>) -> Vec<u8> {
    let marker = |from: usize, jpeg: &[u8]| {
        from + jpeg[from..]
            .windows(2)
            .position(|w| w == [0xFF, 0xE1])
            .unwrap()
    };
    let second = marker(marker(0, &jpeg) + 2, &jpeg);
    jpeg.insert(second, 0x42);
    jpeg
}
