//! An ArtCraft Omni connection saved before Generative fill existed gains it when the store is
//! read, without its model list or anything else changing.
use craft_ai_core::{CapabilityId, ConnectionDraft, ModelId, ModelInfo, PRESETS, StoreFile};

/// A saved connection made from the preset `key`, as the form would save it.
fn saved(key: &str, endpoint: &str) -> craft_ai_core::Connection {
    let preset = PRESETS.iter().find(|p| p.key == key).unwrap();
    let mut draft = ConnectionDraft::from_preset(preset);
    draft.key = "fixture-only-key".to_owned();
    if !endpoint.is_empty() {
        draft.endpoint = endpoint.to_owned();
    }
    draft.prepare(&[]).unwrap().connection
}

/// Fill is added to the saved Omni model while its own label and generation spec stay; a model the
/// adapter does not know, and another provider's model of the same name, are left alone.
#[test]
fn old_omni_connection_gains_fill_and_keeps_everything_else() {
    let fill = CapabilityId::parse("image.inpaint").unwrap();
    let generate = CapabilityId::parse("image.generate").unwrap();
    // What an older build saved: Nano Banana for generation only, plus a model of the person's own.
    let mut omni = saved("artcraft-omni", "");
    omni.models.retain(|m| m.id.as_str() == "nano_banana");
    omni.models[0].label = "My Banana".to_owned();
    omni.models[0]
        .capabilities
        .retain(|s| s.capability == generate);
    omni.models.push(ModelInfo::new(
        ModelId::new("not_documented").unwrap(),
        "Mine",
    ));
    let mut other = saved("custom", "https://example.invalid/v1");
    other.models = vec![ModelInfo::new(
        ModelId::new("nano_banana").unwrap(),
        "Same name",
    )];
    let store = StoreFile {
        connections: vec![omni.clone(), other.clone()],
        ..StoreFile::default()
    };

    let read = StoreFile::parse(&store.to_json().unwrap()).unwrap();

    let banana = &read.connections[0].models[0];
    assert_eq!(
        read.connections[0].models.len(),
        2,
        "no model is added or removed"
    );
    assert_eq!(banana.label, "My Banana");
    assert_eq!(
        banana.capabilities[0], omni.models[0].capabilities[0],
        "the saved spec stays first"
    );
    assert!(banana.spec_for(&fill).is_some(), "Generative fill is added");
    assert_eq!(read.connections[0].models[1], omni.models[1]);
    assert_eq!(read.connections[1], other);
}
