//! Extra coverage for cache identity, malformed state and filesystem boundaries.

use super::*;

/// A relative destination is resolved against the host's current directory, without creating it.
#[test]
fn a_relative_cache_destination_is_supported() {
    assert_eq!(
        destination(Path::new("new-burn-cache")).unwrap(),
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("new-burn-cache")
    );
}

/// A changed expected checkpoint must not silently pick up weights from an older checkpoint.
#[test]
fn a_new_expected_source_never_reuses_previous_weights() {
    let mut r = Rig::new();
    let fake = Fake::default();
    r.prepare(&fake).unwrap();
    r.bundle.files[0].sha256 = "0".repeat(64);
    assert!(r.prepare(&fake).is_err());
    assert_eq!(fake.0.get(), 1);
}

/// A bounded manifest read must not accept arbitrarily padded, otherwise valid JSON.
#[test]
fn an_oversized_manifest_is_refused() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    let manifest = first.encoder.parent().unwrap().join("manifest.json");
    let mut bytes = fs::read(&manifest).unwrap();
    bytes.resize(8192, b' ');
    fs::write(manifest, bytes).unwrap();
    assert!(r.prepare(&fake).is_err());
}

/// A reused conversion is self-contained and does not need its original download present.
#[test]
fn a_complete_conversion_can_be_reused_after_originals_are_removed() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    fs::remove_dir_all(&r.source).unwrap();
    let second = r.prepare(&fake).unwrap();
    assert_eq!(second.preparation, Preparation::Reused);
    assert_eq!(second.encoder, first.encoder);
    assert_eq!(fake.0.get(), 1);
}

/// A new conversion or compiled architecture revision must not reuse earlier tensors.
#[test]
fn a_new_converter_revision_has_its_own_saved_copy() {
    let mut r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    r.bundle.revision.push_str("-next");
    let second = r.prepare(&fake).unwrap();
    assert_eq!(second.preparation, Preparation::Converted);
    assert_ne!(second.encoder, first.encoder);
    assert_eq!(fake.0.get(), 2);
    assert!(first.encoder.is_file());
}

/// Checking a truncated source must fail before entering the converter.
#[test]
fn a_wrong_source_size_is_refused() {
    let mut r = Rig::new();
    r.bundle.files[1].size += 1;
    let fake = Fake::default();
    assert!(r.prepare(&fake).is_err());
    assert_eq!(fake.0.get(), 0);
}

/// A converter failure must leave no partially published directory or loose temporary files.
#[test]
fn conversion_failure_cleans_temporary_files() {
    let r = Rig::new();
    struct Fails;
    impl Converter for Fails {
        /// Writes half a bundle and reports failure.
        fn convert(&self, _: &Path, output: &Path) -> Result<(), AiError> {
            fs::write(output.join("encoder.bpk"), b"partial").unwrap();
            Err(AiError::unsupported("Could not prepare the model."))
        }
    }
    assert!(r.prepare(&Fails).is_err());
    assert!(!r.cache.join(cache_key(&r.bundle)).exists());
    assert!(
        fs::read_dir(&r.cache).unwrap().all(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|x| x == "lock"))
    );
}

/// A library must never place conversion by-products inside the person's source directory.
#[test]
fn the_original_folder_cannot_be_used_as_the_cache() {
    let r = Rig::new();
    assert!(
        prepare(
            &r.bundle,
            &r.source,
            &r.source,
            &r.cancelled,
            &Fake::default()
        )
        .is_err()
    );
    let mut names: Vec<_> = fs::read_dir(&r.source)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["decoder.onnx", "encoder.onnx"]);
}

/// On Unix, a saved component linked outside the managed directory is not a cache entry.
#[test]
#[cfg(unix)]
fn a_linked_saved_component_is_refused() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    let elsewhere = r._dir.path().join("elsewhere.bpk");
    fs::rename(&first.encoder, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &first.encoder).unwrap();
    assert!(r.prepare(&fake).is_err());
}
