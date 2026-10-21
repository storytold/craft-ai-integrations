//! Opt-in check that the catalogue's real files install through the launcher's own downloader.

use craft_ai_comfy_env::{ComfyInstall, DownloadConfig, Downloader, verify_file};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// Fetches the catalogue's Florence-2-large detector into a real ComfyUI folder and checks every
/// file's SHA-256. Needs `CRAFT_TEST_COMFY_BASE` and downloads 1.56 GB, so it runs only on request.
#[test]
#[ignore = "downloads 1.56 GB into a real ComfyUI folder after the owner's approval"]
fn live_florence_detector_installs_from_the_catalogue_with_every_hash_checked() {
    let base = PathBuf::from(std::env::var("CRAFT_TEST_COMFY_BASE").expect("ComfyUI folder"));
    let install = ComfyInstall::at(&base).expect("a ComfyUI folder");
    let features = craft_ai_features::builtin();
    let recipe = features
        .iter()
        .flat_map(|f| &f.locals)
        .find(|r| r.workflow == "sam21-segment")
        .expect("sam21 recipe");
    let dest = install.models_dir_for("LLM").expect("LLM folder");
    let downloader = Downloader::new(DownloadConfig::default());
    let cancel = AtomicBool::new(false);
    let mut fetched = 0;
    for file in recipe.models.iter().filter(|m| m.folder == "LLM") {
        let path = downloader
            .fetch(file, &dest, None, &cancel, &mut |_| {})
            .expect("download");
        verify_file(&path, file, &cancel, &mut |_| {}).expect("sha256 matches the catalogue");
        fetched += 1;
    }
    assert_eq!(fetched, 10);
}
