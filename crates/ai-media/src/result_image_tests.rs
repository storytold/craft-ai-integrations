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
