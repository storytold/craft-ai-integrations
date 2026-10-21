use craft_ai_comfy_env::{ComfyInstall, parse_extra_paths, scan};
use craft_ai_features::{State, builtin, plan};
use std::fs;
use std::path::{Path, PathBuf};

fn touch(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

/// Builds a throwaway ComfyUI folder for the scan tests.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("Comfy");
    touch(&base.join("models/sam3/sam3.safetensors"), 10);
    touch(&base.join("models/checkpoints/a.ckpt"), 5);
    touch(
        &base.join("models/diffusion_models/FLUX/fill.safetensors"),
        6,
    );
    touch(&base.join("models/checkpoints/notes.txt"), 99);
    // A bundle's small config files count; a large dataset file, an unlisted type and a part file do not.
    touch(&base.join("models/LLM/Bundle/config.json"), 12);
    touch(&base.join("models/LLM/Bundle/modeling.py"), 20);
    touch(
        &base.join("models/LLM/Bundle/huge.json"),
        2 * 1024 * 1024 + 1,
    );
    touch(&base.join("models/LLM/Bundle/readme.md"), 30);
    touch(&base.join("models/checkpoints/big.safetensors.part"), 99);
    fs::create_dir_all(base.join("custom_nodes/ComfyUI-SAM3")).unwrap();
    fs::create_dir_all(base.join("custom_nodes/__pycache__")).unwrap();
    fs::create_dir_all(base.join("custom_nodes/.disabled")).unwrap();
    touch(&base.join("custom_nodes/loose_file.py"), 1);
    dir
}

/// A folder with model files is an install; other folders are not.
#[test]
fn a_folder_with_models_is_an_install_and_other_folders_are_not() {
    let dir = tree();
    let base = dir.path().join("Comfy");
    let install = ComfyInstall::at(&base).unwrap();
    let mut folders: Vec<_> = install.model_dirs.iter().map(|(n, _)| n.as_str()).collect();
    folders.sort();
    assert_eq!(folders, ["LLM", "checkpoints", "diffusion_models", "sam3"]);
    assert!(ComfyInstall::at(dir.path().join("nope").as_path()).is_none());
    assert!(ComfyInstall::at(Path::new("relative")).is_none());
    let empty = tempfile::tempdir().unwrap();
    assert!(ComfyInstall::at(empty.path()).is_none());
}

#[test]
/// The scan lists model files small bundle files and node packs.
fn the_scan_lists_model_files_small_bundle_files_and_node_packs() {
    let dir = tree();
    let inv = scan(&ComfyInstall::at(&dir.path().join("Comfy")).unwrap());
    assert!(inv.comfyui_found);
    let mut files: Vec<_> = inv
        .files
        .iter()
        .map(|f| (f.folder.as_str(), f.filename.as_str(), f.size_bytes))
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            ("LLM", "Bundle/config.json", 12),
            ("LLM", "Bundle/modeling.py", 20),
            ("checkpoints", "a.ckpt", 5),
            ("checkpoints", "notes.txt", 99),
            ("diffusion_models", "FLUX/fill.safetensors", 6),
            ("sam3", "sam3.safetensors", 10),
        ]
    );
    assert_eq!(inv.node_folders, ["comfyui-sam3"]);
}

#[test]
fn desktop_config_gives_the_base_folder_and_extra_model_paths() {
    let dir = tree();
    let config = dir.path().join("config");
    let base = dir.path().join("Comfy");
    let shared = dir.path().join("Shared");
    touch(&shared.join("models/vae/v.safetensors"), 3);
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("config.json"),
        format!(
            r#"{{"basePath": {:?}, "installState": "installed"}}"#,
            base.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(
        config.join("extra_models_config.yaml"),
        format!(
            "shared:\n  vae: {}\n  base_path: {}\n",
            "models/vae/",
            shared.to_str().unwrap()
        ),
    )
    .unwrap();
    let install = ComfyInstall::discover_in(&config).unwrap();
    assert_eq!(install.base, base);
    let inv = scan(&install);
    assert!(
        inv.files
            .iter()
            .any(|f| f.folder == "vae" && f.filename == "v.safetensors")
    );
    assert!(inv.files.iter().any(|f| f.folder == "sam3"));
}

#[test]
fn a_missing_or_broken_desktop_config_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ComfyInstall::discover_in(dir.path()).is_none());
    fs::write(dir.path().join("config.json"), "not json").unwrap();
    assert!(ComfyInstall::discover_in(dir.path()).is_none());
    fs::write(
        dir.path().join("config.json"),
        r#"{"basePath": "C:/does/not/exist"}"#,
    )
    .unwrap();
    assert!(ComfyInstall::discover_in(dir.path()).is_none());
    fs::write(dir.path().join("config.json"), r#"{"basePath": 7}"#).unwrap();
    assert!(ComfyInstall::discover_in(dir.path()).is_none());
}

#[test]
fn the_yaml_subset_is_read_in_any_order_and_unsafe_paths_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    for d in [&a, &b, &a.join("loras"), &a.join("two"), &a.join("three")] {
        fs::create_dir_all(d).unwrap();
    }
    let text = format!(
        "# comment\nsec:\n  loras: loras/\n  is_default: \"true\"\n  base_path: {a}\n  multi: |\n    two\n    three\n  gone: nowhere\n  escape: ../b\nother:\n  checkpoints: {b}\n",
        a = a.to_str().unwrap(),
        b = b.to_str().unwrap()
    );
    let mut got = parse_extra_paths(&text);
    got.sort();
    let want: Vec<(String, PathBuf)> = vec![
        ("checkpoints".into(), b.clone()),
        ("loras".into(), a.join("loras")),
        ("multi".into(), a.join("three")),
        ("multi".into(), a.join("two")),
    ];
    assert_eq!(got, want);
    assert!(parse_extra_paths("").is_empty());
    assert!(parse_extra_paths("::::\n  : :\n\t\n").is_empty());
}

#[test]
fn new_files_go_to_the_known_folder_or_a_new_one_and_never_outside() {
    let dir = tree();
    let base = dir.path().join("Comfy");
    let install = ComfyInstall::at(&base).unwrap();
    assert_eq!(
        install.models_dir_for("sam3").unwrap(),
        base.join("models").join("sam3")
    );
    assert_eq!(
        install.models_dir_for("SAM3").unwrap(),
        base.join("models").join("sam3")
    );
    assert_eq!(
        install.models_dir_for("brand_new").unwrap(),
        base.join("models").join("brand_new")
    );
    for bad in ["", "..", "a/b", "a\\b", "C:", "x.y"] {
        assert!(install.models_dir_for(bad).is_none(), "{bad}");
    }
}

#[test]
fn a_planted_file_is_ready_and_a_bare_install_needs_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("Comfy");
    let select = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let model = select.locals[0].models[0].clone();
    fs::create_dir_all(base.join("models")).unwrap();
    fs::create_dir_all(base.join("custom_nodes/comfyui-sam3")).unwrap();
    let install = ComfyInstall::at(&base).unwrap();
    assert_eq!(plan(&select, &scan(&install)).state, State::NeedsInstall);
    // A sparse file of exactly the right size stands in for the 3.4 GB model.
    let path = base.join("models/sam3").join(&model.filename);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(&path)
        .unwrap()
        .set_len(model.size_bytes)
        .unwrap();
    let install = ComfyInstall::at(&base).unwrap();
    assert_eq!(plan(&select, &scan(&install)).state, State::Ready);
}

/// Reads the real ComfyUI Desktop install of the person running the test. Run explicitly:
/// `cargo test -p craft-ai-comfy-env -- --ignored real_install`.
#[test]
#[ignore = "reads this computer's real ComfyUI install"]
fn real_install_is_found_and_select_subject_is_ready() {
    let install = ComfyInstall::discover().expect("ComfyUI Desktop install");
    let inv = scan(&install);
    println!(
        "base {:?}, {} model files, {} node folders, vram {:?}, free disk {:?}",
        install.base,
        inv.files.len(),
        inv.node_folders.len(),
        inv.vram_mb,
        inv.free_disk_bytes
    );
    let select = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let p = plan(&select, &inv);
    println!("{p:#?}");
    assert_eq!(p.state, State::Ready);
}

#[test]
fn symbolic_links_inside_the_models_folder_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("Comfy");
    let outside = dir.path().join("outside");
    touch(&outside.join("secret.safetensors"), 4);
    touch(&base.join("models/sam3/real.safetensors"), 2);
    fs::create_dir_all(base.join("custom_nodes")).unwrap();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, base.join("models/sam3/link"));
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, base.join("models/sam3/link"));
    if linked.is_err() {
        eprintln!("skipped: this account may not create symbolic links");
        return;
    }
    let inv = scan(&ComfyInstall::at(&base).unwrap());
    let names: Vec<_> = inv.files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(names, ["real.safetensors"], "{names:?}");
}
