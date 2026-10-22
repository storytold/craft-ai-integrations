//! Tiny fake conversions prove cache behaviour, never real model execution.

use super::*;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use tempfile::TempDir;

/// A converter whose call count distinguishes a fresh conversion from reuse.
#[derive(Default)]
struct Fake(Cell<usize>);

impl Converter for Fake {
    /// Writes tiny distinguishable weights to the same output shape as the actual converter.
    fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError> {
        self.0.set(self.0.get() + 1);
        assert_eq!(fs::read(source.join("encoder.onnx")).unwrap(), b"encoder");
        assert_eq!(fs::read(source.join("decoder.onnx")).unwrap(), b"decoder");
        fs::write(output.join("encoder.bpk"), b"encoder weights").unwrap();
        fs::write(output.join("decoder.bpk"), b"decoder weights").unwrap();
        Ok(())
    }
}

/// A supported test bundle and original files, separate from the cache folder.
struct Rig {
    _dir: TempDir,
    source: PathBuf,
    cache: PathBuf,
    bundle: Bundle,
    cancelled: AtomicBool,
}

impl Rig {
    /// Builds small source files and freezes their actual hashes.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("originals");
        fs::create_dir(&source).unwrap();
        let files = ["encoder", "decoder"].map(|name| {
            let bytes = name.as_bytes();
            let filename = format!("{name}.onnx");
            fs::write(source.join(&filename), bytes).unwrap();
            SourceFile {
                name: filename,
                size: bytes.len() as u64,
                sha256: Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            }
        });
        Self {
            cache: dir.path().join("cache"),
            _dir: dir,
            source,
            bundle: Bundle {
                revision: "test-v1".into(),
                files,
            },
            cancelled: AtomicBool::new(false),
        }
    }

    /// Runs the production cache lifecycle with the supplied tiny converter.
    fn prepare(&self, converter: &dyn Converter) -> Result<PreparedWeights, AiError> {
        prepare(
            &self.bundle,
            &self.source,
            &self.cache,
            &self.cancelled,
            converter,
        )
    }
}

/// An original file with the wrong bytes must be refused before conversion.
#[test]
fn a_source_hash_mismatch_never_reaches_the_converter() {
    let mut r = Rig::new();
    r.bundle.files[0].sha256 = "0".repeat(64);
    let fake = Fake::default();
    let error = r.prepare(&fake).unwrap_err();
    assert!(error.message.contains("original model files"), "{error:?}");
    assert_eq!(fake.0.get(), 0);
}

/// A saved file changed without changing its size must not be reused.
#[test]
fn damaged_saved_weights_are_refused_without_overwriting_them() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    fs::write(&first.encoder, b"Encoder weights").unwrap();
    let error = r.prepare(&fake).unwrap_err();
    assert_eq!(error.message, DAMAGED_COPY);
    assert_eq!(fake.0.get(), 1);
    assert_eq!(fs::read(first.encoder).unwrap(), b"Encoder weights");
}

/// One way of damaging a saved copy, given the path of its manifest.
type Damage = fn(&Path);

/// Whatever is wrong with a saved copy (unreadable manifest, another conversion's manifest, a
/// manifest padded past its limit), the answer says the copy is damaged and what to do, not the
/// "try again" that can never succeed. The copy is left exactly as it was.
#[test]
fn every_kind_of_damaged_saved_copy_gets_the_same_plain_answer() {
    let corrupt: [(&str, Damage); 3] = [
        ("not json", |m| fs::write(m, b"not json").unwrap()),
        ("another key", |m| {
            let text = fs::read_to_string(m).unwrap();
            let key = text.split('"').nth(3).unwrap().to_owned();
            fs::write(m, text.replace(&key, &"0".repeat(key.len()))).unwrap();
        }),
        ("padded", |m| {
            let mut bytes = fs::read(m).unwrap();
            bytes.resize(8192, b' ');
            fs::write(m, bytes).unwrap();
        }),
    ];
    for (name, damage) in corrupt {
        let r = Rig::new();
        let fake = Fake::default();
        let first = r.prepare(&fake).unwrap();
        let manifest = first.encoder.parent().unwrap().join("manifest.json");
        damage(&manifest);
        let before = fs::read(&manifest).unwrap();
        let error = r.prepare(&fake).unwrap_err();
        assert_eq!(error.message, DAMAGED_COPY, "{name}");
        assert_eq!(
            fs::read(&manifest).unwrap(),
            before,
            "{name}: left as it was"
        );
        assert_eq!(fake.0.get(), 1, "{name}: never converted again");
    }
}

/// A published directory that has lost a component is not ready.
#[test]
fn a_missing_saved_decoder_is_not_a_complete_conversion() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    fs::remove_file(first.decoder).unwrap();
    assert!(r.prepare(&fake).is_err());
    assert_eq!(fake.0.get(), 1);
}

/// Preparing once saves checked weights and the second request reuses them without converting.
#[test]
fn choosing_burn_again_reuses_the_local_weights() {
    let r = Rig::new();
    let fake = Fake::default();
    let first = r.prepare(&fake).unwrap();
    assert_eq!(first.preparation, Preparation::Converted);
    assert_eq!(fs::read(&first.encoder).unwrap(), b"encoder weights");
    assert_eq!(fs::read(&first.decoder).unwrap(), b"decoder weights");
    let second = r.prepare(&fake).unwrap();
    assert_eq!(second.preparation, Preparation::Reused);
    assert_eq!(second.encoder, first.encoder);
    assert_eq!(fake.0.get(), 1);
    assert_eq!(fs::read(r.source.join("encoder.onnx")).unwrap(), b"encoder");
    assert_eq!(fs::read(r.source.join("decoder.onnx")).unwrap(), b"decoder");
}

/// A run that was killed leaves its scratch folders behind. The next preparation of the same files
/// removes them, and leaves alone the scratch folders of any other cache key and any other folder
/// that merely starts with this key (a damaged copy a person renamed to `<key>.backup`).
#[test]
fn leftovers_of_a_killed_run_are_swept_for_this_key_only() {
    let r = Rig::new();
    let key = cache_key(&r.bundle);
    fs::create_dir_all(&r.cache).unwrap();
    let mine = r.cache.join(format!("{key}.work.killed"));
    let other = r
        .cache
        .join(format!("{}.work.killed", "f".repeat(key.len())));
    for dir in [&mine, &other] {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("encoder.onnx"), b"leftover").unwrap();
    }
    let backup = r.cache.join(format!("{key}.backup"));
    fs::create_dir(&backup).unwrap();
    fs::write(backup.join("encoder.bpk"), b"kept").unwrap();
    r.prepare(&Fake::default()).unwrap();
    assert!(!mine.exists(), "this key's leftovers are removed");
    assert!(backup.exists(), "a renamed copy is not scratch");
    assert!(
        other.exists(),
        "another key's leftovers are not ours to remove"
    );
}

#[path = "boundaries.rs"]
mod boundaries;
#[path = "safety.rs"]
mod safety;
