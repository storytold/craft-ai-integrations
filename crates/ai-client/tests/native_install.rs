//! Native installation boundaries, using tiny files and refused or scripted local sources only.
use std::sync::atomic::AtomicBool;

use craft_ai_client::NativeInstall;
use craft_ai_comfy_env::{DownloadConfig, Downloader};
use craft_ai_core::ErrorKind;

mod native_setup_support;
use native_setup_support::{candle, candle_mut, feature, sam3_mut, world};

/// A recipe marked unusable gives no file list and no download destination.
#[test]
fn an_unusable_recipe_offers_neither_files_nor_a_download_destination() {
    let (_dir, desk, _) = world();
    let mut feature = feature();
    sam3_mut(&mut feature).usable = false;
    assert!(desk.native_download_dir(&feature, "candle").is_err());
    assert!(desk.native_files(&feature, "candle").is_err());
}

/// An unusable recipe in front does not hide the usable one behind it: files are found for, and a
/// destination is given to, the first recipe that can run.
#[test]
fn an_unusable_first_recipe_does_not_hide_a_usable_one_from_file_setup() {
    let (_dir, desk, _) = world();
    let mut feature = feature();
    let mut parked = sam3_mut(&mut feature).clone();
    parked.usable = false;
    parked.native.clear();
    feature.locals.insert(0, parked);
    assert!(desk.native_download_dir(&feature, "candle").is_ok());
    assert!(desk.native_files(&feature, "candle").is_ok());
}

/// A cancel set before the install starts means no request and no destination folder.
#[test]
fn cancelled_before_start_neither_contacts_a_source_nor_creates_a_destination() {
    let (_dir, desk, _) = world();
    let feature = feature();
    let destination = desk.native_download_dir(&feature, "candle").unwrap();
    let downloader = Downloader::new(DownloadConfig::default());
    let cancel = AtomicBool::new(true);
    let error = desk
        .install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut |_, _| {},
            },
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Cancelled);
    assert!(!destination.exists());
}

/// Existing files are hashed, and only a matching set yields a weights path.
#[test]
fn existing_files_are_hashed_before_the_weights_path_can_be_enabled() {
    let (_dir, desk, models) = world();
    let feature = feature();
    let directory = models.join("personal-models");
    std::fs::create_dir(&directory).unwrap();
    for file in &candle(&feature).models {
        std::fs::write(
            directory.join(&file.filename),
            vec![b'x'; file.size_bytes as usize],
        )
        .unwrap();
    }
    let cancel = AtomicBool::new(false);
    assert_eq!(
        desk.verify_native_files(&feature, "candle", &cancel, &mut |_, _| {})
            .unwrap(),
        directory.join("model.safetensors")
    );
    std::fs::write(directory.join("vocab.json"), b"yy").unwrap();
    let error = desk
        .verify_native_files(&feature, "candle", &cancel, &mut |_, _| {})
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    assert_eq!(
        std::fs::read(directory.join("vocab.json")).unwrap(),
        b"yy",
        "checking a person's files never repairs or replaces them"
    );
}

/// A recipe filename that climbs out of the model directory is refused and the outside file is not read.
#[test]
fn a_recipe_filename_cannot_escape_the_discovered_model_directory() {
    let (_dir, desk, models) = world();
    let mut feature = feature();
    let directory = models.join("personal-models");
    std::fs::create_dir(&directory).unwrap();
    for file in &candle(&feature).models {
        std::fs::write(
            directory.join(&file.filename),
            vec![b'x'; file.size_bytes as usize],
        )
        .unwrap();
    }
    std::fs::write(models.join("outside.safetensors"), b"x").unwrap();
    candle_mut(&mut feature).models[0].filename = "../outside.safetensors".into();
    let error = desk
        .verify_native_files(&feature, "candle", &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidRequest);
    assert!(desk.native_download_dir(&feature, "candle").is_err());
}
