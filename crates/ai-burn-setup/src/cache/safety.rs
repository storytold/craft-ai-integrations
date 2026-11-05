//! Negative lifecycle checks for a local, cross-app cache.

use super::*;
use craft_ai_core::ErrorKind;
use std::sync::atomic::Ordering;

/// Cancellation already given must leave even the destination directory absent.
#[test]
fn cancel_before_start_writes_nothing() {
    let r = Rig::new();
    r.cancelled.store(true, Ordering::Release);
    let fake = Fake::default();
    assert_eq!(r.prepare(&fake).unwrap_err().kind, ErrorKind::Cancelled);
    assert!(!r.cache.exists());
    assert_eq!(fake.0.get(), 0);
}

/// Conversion may finish after Cancel, but its files must never become a saved ready copy.
#[test]
fn cancel_during_conversion_does_not_publish_a_result() {
    let r = Rig::new();
    struct Cancel<'a>(&'a AtomicBool);
    impl Converter for Cancel<'_> {
        /// Simulates Cancel arriving during the synchronous third-party converter.
        fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError> {
            Fake::default().convert(source, output)?;
            self.0.store(true, Ordering::Release);
            Ok(())
        }
    }
    assert_eq!(
        r.prepare(&Cancel(&r.cancelled)).unwrap_err().kind,
        ErrorKind::Cancelled
    );
    assert!(!r.cache.join(cache_key(&r.bundle)).exists());
    assert!(
        fs::read_dir(&r.cache).unwrap().all(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|x| x == "lock"))
    );
}

/// Two apps cannot convert the same identity at the same time.
#[test]
fn another_apps_conversion_lock_is_respected() {
    let r = Rig::new();
    fs::create_dir(&r.cache).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(r.cache.join(format!("{}.lock", cache_key(&r.bundle))))
        .unwrap();
    lock.try_lock().unwrap();
    let fake = Fake::default();
    let error = r.prepare(&fake).unwrap_err();
    assert!(error.message.contains("Another app"), "{error:?}");
    assert_eq!(fake.0.get(), 0);
    drop(lock);
    assert_eq!(
        r.prepare(&fake).unwrap().preparation,
        Preparation::Converted
    );
}

/// Generated source is not something to compile or retain on the person's machine.
#[test]
fn only_weights_and_the_manifest_are_kept() {
    let r = Rig::new();
    struct Extra;
    impl Converter for Extra {
        /// A converter can emit source as well, but it is a temporary by-product only.
        fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError> {
            Fake::default().convert(source, output)?;
            fs::write(output.join("encoder.rs"), b"not compiled").unwrap();
            Ok(())
        }
    }
    let weights = r.prepare(&Extra).unwrap();
    let mut names: Vec<_> = fs::read_dir(weights.encoder.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["decoder.bpk", "encoder.bpk", "manifest.json"]);
}

/// A third-party converter panic is reported, not propagated into a host app.
#[test]
fn a_converter_panic_leaves_no_ready_copy() {
    let r = Rig::new();
    struct Crashes;
    impl Converter for Crashes {
        /// Simulates a dependency that panics instead of returning a result.
        fn convert(&self, _: &Path, _: &Path) -> Result<(), AiError> {
            panic!("converter failed");
        }
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.prepare(&Crashes)));
    assert!(result.is_ok(), "the dependency panic escaped");
    let error = result.unwrap().unwrap_err();
    assert!(error.message.contains("prepare"));
    assert!(!r.cache.join(cache_key(&r.bundle)).exists());
}

/// An empty component cannot be published even when the converter reports success.
#[test]
fn empty_converted_weights_are_refused() {
    let r = Rig::new();
    struct Empty;
    impl Converter for Empty {
        /// A successful return with an empty file is not a successful conversion.
        fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError> {
            Fake::default().convert(source, output)?;
            fs::write(output.join("encoder.bpk"), b"").unwrap();
            Ok(())
        }
    }
    assert!(r.prepare(&Empty).is_err());
    assert!(!r.cache.join(cache_key(&r.bundle)).exists());
}
