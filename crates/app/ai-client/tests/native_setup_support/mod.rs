//! Tiny local-model fixtures. No real host or model weights are used.
use craft_ai_client::FolderDesk;
use craft_ai_features::{Feature, LocalRecipe, NativeRoute, Source, builtin};
use craft_ai_native::shared::SharedStore;
use std::path::PathBuf;

/// The built-in `select-subject` feature with Candle's three files given small sizes, known hashes and a loopback source the default downloader refuses.
pub fn feature() -> Feature {
    let mut feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let hashes = [
        "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        "5dde896887f6754c9b15bfe3a441ae4806df2fde94001311e08bf110622e0bbe",
        "cd2eb0837c9b4c962c22d2ff8b5441b7b45805887f051d39bf133b583baf6860",
    ];
    for (index, model) in candle_mut(&mut feature).models.iter_mut().enumerate() {
        model.size_bytes = (index + 1) as u64;
        model.sha256 = hashes[index].into();
        // The default downloader refuses this loopback URL before it can open a connection.
        model.source = Source::Url {
            url: "http://127.0.0.1:1/not-contacted".into(),
        };
    }
    feature
}

/// A temporary shared store with one registered model folder, and the desk that owns it.
pub fn world() -> (tempfile::TempDir, FolderDesk, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    std::fs::create_dir(&models).unwrap();
    let desk = FolderDesk::new(SharedStore::new(dir.path().join("connections.json")));
    let chosen = desk.add(&models, None).unwrap();
    desk.set_default(&chosen.folder.id).unwrap();
    (dir, desk, models)
}

/// The Candle route of `feature`, from that engine's own recipe (SAM 3), never by its place in
/// the list: SAM 2.1 comes first and lists only rten.
#[allow(dead_code)] // Not every test binary that includes this module uses every fixture.
pub fn candle(feature: &Feature) -> &NativeRoute {
    feature
        .engine_recipe("candle")
        .and_then(|r| r.native.iter().find(|n| n.provider == "candle"))
        .unwrap()
}

/// [`candle`], to change for one test.
#[allow(dead_code)] // Not every test binary that includes this module uses every fixture.
pub fn candle_mut(feature: &mut Feature) -> &mut NativeRoute {
    sam3_mut(feature)
        .native
        .iter_mut()
        .find(|n| n.provider == "candle")
        .unwrap()
}

/// The SAM 3 recipe of `feature`, found by its workflow name.
#[allow(dead_code)] // Not every test binary that includes this module uses every fixture.
pub fn sam3_mut(feature: &mut Feature) -> &mut LocalRecipe {
    feature
        .locals
        .iter_mut()
        .find(|r| r.workflow == "sam3-segment")
        .unwrap()
}
