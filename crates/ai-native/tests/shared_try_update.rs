//! Fallible locked updates commit only after a successful callback and validation.
use std::cell::Cell;

use craft_ai_core::{Connection, StoreFile, store::StoreError};
use craft_ai_native::shared::{SharedError, SharedStore};

/// A valid fixture connection without credentials or a live service.
fn connection(id: &str) -> Connection {
    serde_json::from_value(serde_json::json!({
        "id":id, "label":id, "provider":"fixture", "endpoint":"https://fixture.example.invalid",
        "locality":"remote", "auth":{"kind":"none"}, "billing":"free", "models":[]
    }))
    .unwrap()
}

/// A raw same-schema file with an unknown nested setting and external formatting.
fn external_bytes() -> Vec<u8> {
    let mut raw = serde_json::to_value(StoreFile {
        connections: vec![connection("original")],
        ..StoreFile::default()
    })
    .unwrap();
    raw["connections"][0]["future_transport"] = serde_json::json!({"keep":true});
    raw["future_root"] = serde_json::json!({"keep":true});
    format!("\n{}\n\n", serde_json::to_string(&raw).unwrap()).into_bytes()
}

/// Callback failure preserves exact bytes even after the callback mutates its in-memory copy.
#[test]
fn callback_error_preserves_raw_nested_settings_and_discards_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(dir.path().join("connections.json"));
    let before = external_bytes();
    std::fs::write(store.path(), &before).unwrap();
    let result = store
        .try_update(|s| {
            s.connections.clear();
            Err::<(), _>("do not commit")
        })
        .unwrap();
    assert_eq!(result, Err("do not commit"));
    assert!(
        std::fs::read(store.path()).unwrap() == before,
        "a failed callback rewrote the raw store"
    );
}

/// A rejected callback leaves a missing store absent and releases its lock for a later writer.
#[test]
fn callback_error_does_not_create_a_missing_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(dir.path().join("sub/connections.json"));
    assert_eq!(
        store.try_update(|_| Err::<(), _>("stop")).unwrap(),
        Err("stop")
    );
    assert!(!store.path().exists());
    store
        .try_update(|s| {
            s.connections.push(connection("later"));
            Ok::<_, ()>(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(store.load().unwrap().connections[0].id.as_str(), "later");
}

/// Successful callbacks return their value, commit valid changes and retain unknown top-level fields.
#[test]
fn successful_callback_commits_and_returns_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(dir.path().join("connections.json"));
    std::fs::write(store.path(), external_bytes()).unwrap();
    let count = store
        .try_update(|s| {
            s.connections.push(connection("second"));
            Ok::<_, ()>(s.connections.len())
        })
        .unwrap()
        .unwrap();
    assert_eq!(count, 2);
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(raw["future_root"]["keep"], true);
    assert_eq!(store.load().unwrap().connections.len(), 2);
}

/// Invalid changes are refused before serialisation, separately from callback errors.
#[test]
fn validation_failure_does_not_commit_successful_callback_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(dir.path().join("connections.json"));
    let before = external_bytes();
    std::fs::write(store.path(), &before).unwrap();
    let result = store.try_update(|s| {
        s.connections.push(connection("original"));
        Ok::<_, ()>(())
    });
    assert!(matches!(
        result,
        Err(SharedError::Store(StoreError::DuplicateConnection(_)))
    ));
    assert!(std::fs::read(store.path()).unwrap() == before);
}

/// Damaged and newer-schema stores refuse the update before invoking its callback.
#[test]
fn read_errors_never_call_the_callback_or_rewrite_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(dir.path().join("connections.json"));
    for before in ["{ truncated", "{\"schema_version\":99}"] {
        std::fs::write(store.path(), before).unwrap();
        let called = Cell::new(false);
        let result = store.try_update(|_| {
            called.set(true);
            Ok::<_, ()>(())
        });
        assert!(result.is_err());
        assert!(!called.get());
        assert_eq!(std::fs::read_to_string(store.path()).unwrap(), before);
    }
}

/// Independent handles share the same lock, so successful concurrent updates all survive.
#[test]
fn concurrent_fallible_writers_all_land() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                SharedStore::new(path).try_update(|s| {
                    s.connections.push(connection(&format!("writer-{i}")));
                    Ok::<_, ()>(())
                })
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap().unwrap().unwrap();
    }
    assert_eq!(SharedStore::new(path).load().unwrap().connections.len(), 8);
}
