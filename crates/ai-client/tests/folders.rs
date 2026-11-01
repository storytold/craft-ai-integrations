//! The model folder list through the shared store, as the launcher and an app would use it.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use craft_ai_client::{FolderDesk, NativeInstall};
use craft_ai_comfy_env::{DownloadConfig, Downloader, parse_extra_paths};
use craft_ai_core::model_folders::{FolderOrigin, ModelFolder};
use craft_ai_features::{State, builtin, plan_native};
use craft_ai_native::shared::SharedStore;

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    models: tempfile::TempDir,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let models = tempfile::tempdir().unwrap();
    write(models.path(), "vae/ae.safetensors", b"x");
    write(models.path(), "Lora/style.safetensors", b"x");
    World {
        _dir: dir,
        shared,
        models,
    }
}

fn desk(w: &World) -> FolderDesk {
    FolderDesk::new(w.shared.clone())
}

fn listing(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            out.push(format!(
                "{} {}",
                e.path().display(),
                e.metadata().unwrap().len()
            ));
            if e.path().is_dir() {
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// Discovery registers the folder without selecting it for downloads.
#[test]
fn a_linked_folder_is_read_and_stored_without_becoming_default() {
    let w = world();
    let added = desk(&w).add(w.models.path(), Some("My models")).unwrap();
    assert_eq!(added.folder.id, "my-models");
    assert!(!added.folder.default && added.folder.origin == FolderOrigin::Chosen);
    assert_eq!(added.folder.kinds["vae"], ["vae"]);
    assert_eq!(added.folder.kinds["loras"], ["Lora"]);
    assert_eq!(added.scan.files.len(), 2);
    // Another app reading the same store sees it.
    let other = FolderDesk::new(SharedStore::new(w.shared.path()))
        .list()
        .unwrap();
    assert_eq!(other, vec![added.folder]);
}

#[test]
fn nothing_in_the_folder_is_changed_by_adding_rescanning_or_removing() {
    let w = world();
    let before = listing(w.models.path());
    let d = desk(&w);
    let id = d.add(w.models.path(), None).unwrap().folder.id;
    d.rescan(&id).unwrap();
    d.remove(&id).unwrap();
    assert_eq!(listing(w.models.path()), before);
}

#[test]
fn the_same_folder_cannot_be_added_twice_however_it_is_spelled() {
    let w = world();
    let d = desk(&w);
    d.add(w.models.path(), None).unwrap();
    let with_slash = format!("{}{}", w.models.path().display(), std::path::MAIN_SEPARATOR);
    let err = d.add(Path::new(&with_slash), None).unwrap_err();
    assert!(err.message.contains("already"), "{}", err.message);
    assert_eq!(d.list().unwrap().len(), 1);
}

#[test]
fn something_that_is_not_a_folder_is_refused_and_nothing_is_stored() {
    let w = world();
    let d = desk(&w);
    assert!(
        d.add(&w.models.path().join("vae/ae.safetensors"), None)
            .is_err()
    );
    assert!(d.add(&w.models.path().join("missing"), None).is_err());
    assert!(d.add(Path::new("relative"), None).is_err());
    assert!(d.list().unwrap().is_empty());
}

#[test]
fn a_rescan_sees_what_was_added_since() {
    let w = world();
    let d = desk(&w);
    let id = d.add(w.models.path(), None).unwrap().folder.id;
    write(w.models.path(), "unet/flux.gguf", b"x");
    let scan = d.rescan(&id).unwrap();
    assert_eq!(scan.files.len(), 3);
    assert_eq!(d.list().unwrap()[0].kinds["diffusion_models"], ["unet"]);
}

/// Only an explicitly chosen folder may become the default.
#[test]
fn the_default_moves_and_a_linked_folder_can_never_be_it() {
    let w = world();
    let d = desk(&w);
    let second = tempfile::tempdir().unwrap();
    let first = d.add(w.models.path(), Some("one")).unwrap().folder;
    let two = d.add(second.path(), Some("two")).unwrap().folder;
    d.set_default(&first.id).unwrap();
    assert!(!first.default && !two.default);
    d.set_default(&two.id).unwrap();
    let now = d.list().unwrap();
    assert_eq!(
        now.iter()
            .filter(|f| f.default)
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>(),
        ["two"]
    );
    w.shared
        .update(|s| {
            s.model_folders.push(ModelFolder {
                id: "comfy".into(),
                label: "ComfyUI".into(),
                path: second.path().to_string_lossy().into_owned(),
                default: false,
                origin: FolderOrigin::ComfyUi,
                kinds: Default::default(),
            });
        })
        .unwrap();
    assert!(
        d.set_default("comfy").is_err(),
        "a read only folder cannot receive downloads"
    );
    assert!(d.set_default("nope").is_err());
}

/// Removing a default never turns a discovery-only folder into a destination.
#[test]
fn removing_the_default_does_not_promote_a_linked_folder() {
    let w = world();
    let d = desk(&w);
    let other = tempfile::tempdir().unwrap();
    let first = d.add(w.models.path(), Some("one")).unwrap().folder;
    d.add(other.path(), Some("two")).unwrap();
    d.set_default(&first.id).unwrap();
    d.remove(&first.id).unwrap();
    let left = d.list().unwrap();
    assert_eq!(left.len(), 1);
    assert!(!left[0].default, "a linked folder is not silently promoted");
    assert!(d.remove(&first.id).is_err());
}

#[test]
fn the_extra_paths_file_matches_the_folders_and_is_replaced_not_added_to() {
    let w = world();
    let d = desk(&w);
    d.add(w.models.path(), Some("one")).unwrap();
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("ai").join("comfy_extra_paths.yaml");
    d.write_extra_paths(&file).unwrap();
    let mut places = parse_extra_paths(&std::fs::read_to_string(&file).unwrap());
    places.sort();
    assert_eq!(
        places,
        vec![
            ("loras".to_owned(), w.models.path().join("Lora")),
            ("vae".to_owned(), w.models.path().join("vae"))
        ]
    );
    write(w.models.path(), "unet/flux.gguf", b"x");
    let id = d.list().unwrap()[0].id.clone();
    d.rescan(&id).unwrap();
    d.write_extra_paths(&file).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("diffusion_models")
    );
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["comfy_extra_paths.yaml"],
        "no temporary file is left behind"
    );
}

/// A plain add never creates a folder; `add_new` does, and only for an absolute path.
#[test]
fn a_proposed_folder_that_does_not_exist_yet_is_made_when_accepted() {
    let w = world();
    let d = desk(&w);
    let proposed = w.models.path().join("ArtCraft").join("models");
    assert!(
        d.add(&proposed, None).is_err(),
        "a plain add never creates anything"
    );
    assert!(!proposed.exists());
    let added = d.add_new(&proposed, Some("ArtCraft models")).unwrap();
    assert!(proposed.is_dir() && added.folder.default && added.scan.files.is_empty());
    assert!(d.add_new(Path::new("relative/models"), None).is_err());
}

/// A known tokenizer file is recognised by name and size where it sits, and is not offered to ComfyUI.
#[test]
fn a_known_native_tokenizer_file_is_mapped_without_renaming_it() {
    let w = world();
    let native = w.models.path().join("any-layout");
    std::fs::create_dir_all(&native).unwrap();
    let file = std::fs::File::create(native.join("merges.txt")).unwrap();
    file.set_len(524_619).unwrap();
    let d = desk(&w);
    let added = d.add(w.models.path(), None).unwrap();
    assert!(
        added
            .scan
            .files
            .iter()
            .any(|f| f.rel == "any-layout/merges.txt"
                && f.kind.is_none()
                && f.basis == Some(craft_ai_comfy_env::Basis::Recipe))
    );
    assert!(
        !added.folder.kinds.contains_key("sam3"),
        "the incompatible weights must not be offered to ComfyUI"
    );
}

#[test]
fn a_very_long_folder_name_is_still_accepted_with_a_shortened_label() {
    let w = world();
    let long = w.models.path().join("m".repeat(130));
    write(&long, "vae/a.safetensors", b"x");
    let added = desk(&w).add(&long, None).unwrap();
    assert_eq!(added.folder.label.chars().count(), 120);
}

// macOS refuses a file name that is not valid text, so the test cannot even make its folder there.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn a_path_that_is_not_valid_text_is_refused_not_stored_as_another_folder() {
    use std::os::unix::ffi::OsStrExt;
    let w = world();
    let odd = w
        .models
        .path()
        .join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    std::fs::create_dir_all(&odd).unwrap();
    assert!(desk(&w).add(&odd, None).is_err());
    assert!(desk(&w).list().unwrap().is_empty());
}

/// Removing the only folder leaves an empty list, for this desk and for a second reader of the file.
#[test]
fn removing_the_last_folder_leaves_none_not_the_old_list() {
    let w = world();
    let d = desk(&w);
    let id = d.add(w.models.path(), None).unwrap().folder.id;
    assert_eq!(d.list().unwrap().len(), 1);
    d.remove(&id).unwrap();
    assert!(d.list().unwrap().is_empty());
    // A second reader of the same file sees the same.
    assert!(
        SharedStore::new(w.shared.path())
            .load()
            .unwrap()
            .model_folders
            .is_empty()
    );
}

/// The Candle route of `feature`, from that engine's own recipe (SAM 3), never by position.
fn candle(feature: &craft_ai_features::Feature) -> &craft_ai_features::NativeRoute {
    feature
        .engine_recipe("candle")
        .and_then(|r| r.native.iter().find(|n| n.provider == "candle"))
        .unwrap()
}

/// [`candle`], to change for one test.
fn candle_mut(feature: &mut craft_ai_features::Feature) -> &mut craft_ai_features::NativeRoute {
    feature
        .locals
        .iter_mut()
        .find(|r| r.workflow == "sam3-segment")
        .and_then(|r| r.native.iter_mut().find(|n| n.provider == "candle"))
        .unwrap()
}

/// The built-in `select-subject` feature with Candle's files given tiny sizes, so tests can write real files cheaply.
fn small_native_feature() -> craft_ai_features::Feature {
    let mut feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let route = candle_mut(&mut feature);
    for (index, model) in route.models.iter_mut().enumerate() {
        model.size_bytes = (index + 1) as u64;
    }
    feature
}

/// A complete set of native files is found in any folder layout, with no ComfyUI folder involved.
#[test]
fn finds_a_complete_native_model_in_an_arbitrary_layout_without_comfyui() {
    let w = world();
    let feature = small_native_feature();
    let directory = w.models.path().join("personal/models/hugging-face");
    for model in &candle(&feature).models {
        write(
            &directory,
            &model.filename,
            &vec![b'x'; model.size_bytes as usize],
        );
    }
    let d = desk(&w);
    d.add(w.models.path(), None).unwrap();
    let found = d.native_files(&feature, "candle").unwrap();
    assert_eq!(found.directory, Some(directory));
    assert_eq!(
        plan_native(&feature, "candle", &found.inventory).state,
        State::Ready
    );
}

/// Files split across two directories never count as one working model; the plan asks for the missing one.
#[test]
fn native_files_in_separate_directories_do_not_count_as_a_working_model() {
    let w = world();
    let feature = small_native_feature();
    let models = &candle(&feature).models;
    let a = w.models.path().join("first");
    let b = w.models.path().join("second");
    write(&a, &models[0].filename, b"x");
    for model in &models[1..] {
        write(&b, &model.filename, &vec![b'x'; model.size_bytes as usize]);
    }
    let d = desk(&w);
    d.add(w.models.path(), None).unwrap();
    let found = d.native_files(&feature, "candle").unwrap();
    assert_eq!(found.directory, Some(b));
    let plan = plan_native(&feature, "candle", &found.inventory);
    assert_eq!(plan.state, State::NeedsInstall);
    assert_eq!(plan.missing_files.len(), 1);
    assert_eq!(plan.missing_files[0].filename, "model.safetensors");
}

/// A registered folder that has gone (an unplugged drive, a deleted folder) is not a candidate. It
/// does not stop discovery in the folders that remain.
#[test]
fn a_registered_folder_that_has_gone_does_not_stop_native_discovery() {
    let w = world();
    let feature = small_native_feature();
    let directory = w.models.path().join("kept");
    for model in &candle(&feature).models {
        write(
            &directory,
            &model.filename,
            &vec![b'x'; model.size_bytes as usize],
        );
    }
    let d = desk(&w);
    d.add(w.models.path(), None).unwrap();
    let gone = tempfile::tempdir().unwrap();
    d.add(gone.path(), None).unwrap();
    drop(gone);
    let found = d.native_files(&feature, "candle").unwrap();
    assert_eq!(found.directory, Some(directory));
}

/// A native download goes only to the chosen default folder, never to a folder linked from ComfyUI, and asking for the path creates nothing.
#[test]
fn native_downloads_go_only_to_the_chosen_default_not_a_linked_folder() {
    let w = world();
    let d = desk(&w);
    let feature = small_native_feature();
    assert!(d.native_download_dir(&feature, "candle").is_err());
    d.add(w.models.path(), None).unwrap();
    w.shared
        .update(|store| {
            store.model_folders[0].origin = FolderOrigin::ComfyUi;
            store.model_folders[0].default = true;
        })
        .unwrap();
    assert!(d.native_download_dir(&feature, "candle").is_err());
    w.shared
        .update(|store| store.model_folders[0].default = false)
        .unwrap();
    let chosen = tempfile::tempdir().unwrap();
    let chosen_folder = d.add(chosen.path(), None).unwrap();
    d.set_default(&chosen_folder.folder.id).unwrap();
    let dir = d.native_download_dir(&feature, "candle").unwrap();
    assert_eq!(dir, chosen.path().join("sam3_transformers"));
    assert!(
        !dir.exists(),
        "asking for a destination does not begin an install"
    );
}

/// An install refuses without licence approval, and files already in place are hashed again before they count.
#[test]
fn native_install_requires_licence_and_rehashes_existing_files() {
    let w = world();
    let d = desk(&w);
    let selected = d.add(w.models.path(), None).unwrap();
    d.set_default(&selected.folder.id).unwrap();
    let mut feature = small_native_feature();
    let hashes = [
        "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        "5dde896887f6754c9b15bfe3a441ae4806df2fde94001311e08bf110622e0bbe",
        "cd2eb0837c9b4c962c22d2ff8b5441b7b45805887f051d39bf133b583baf6860",
    ];
    for (model, hash) in candle_mut(&mut feature).models.iter_mut().zip(hashes) {
        model.sha256 = hash.into();
    }
    let destination = d.native_download_dir(&feature, "candle").unwrap();
    let downloader = Downloader::new(DownloadConfig::default());
    let cancel = AtomicBool::new(false);
    let mut progress = |_: usize, _: craft_ai_comfy_env::Progress| {};
    assert!(
        d.install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: false,
                cancel: &cancel,
                progress: &mut progress,
            }
        )
        .is_err()
    );
    assert!(
        !destination.exists(),
        "declining a licence must leave the disk untouched"
    );
    for model in &candle(&feature).models {
        write(
            &destination,
            &model.filename,
            &vec![b'x'; model.size_bytes as usize],
        );
    }
    assert_eq!(
        d.install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut progress,
            }
        )
        .unwrap(),
        destination
    );
    write(&destination, "vocab.json", b"yy");
    assert!(
        d.install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut progress,
            }
        )
        .is_err(),
        "an existing same-size file must not bypass its checksum"
    );
}
