//! An ArtCraft Omni fill saved before reference pictures were mapped gains them, as an optional
//! input, when the store is read. Nothing it already says is changed.
use craft_ai_core::{
    CapabilityId, CapabilitySpec, ConnectionDraft, InputRole, ModelId, PRESETS, StoreFile,
};

/// The Omni preset as the form would save it, with its fill spec replaced by `fill`.
fn saved_with_fill(fill: CapabilitySpec) -> craft_ai_core::Connection {
    let preset = PRESETS.iter().find(|p| p.key == "artcraft-omni").unwrap();
    let mut draft = ConnectionDraft::from_preset(preset);
    draft.key = "fixture-only-key".to_owned();
    let mut c = draft.prepare(&[]).unwrap().connection;
    c.models.retain(|m| m.id.as_str() == "nano_banana_2");
    let specs = &mut c.models[0].capabilities;
    specs.retain(|s| s.capability != fill.capability);
    specs.push(fill);
    c
}

/// The fill spec an earlier build saved: source, mask and prompt, no references.
fn old_fill() -> CapabilitySpec {
    CapabilitySpec::new(CapabilityId::parse("image.inpaint").unwrap())
        .accepting(InputRole::Source, 1)
        .requiring(InputRole::Source)
        .accepting(InputRole::Mask, 1)
        .requiring(InputRole::Mask)
        .accepting(InputRole::Prompt, 1)
}

/// The saved fill gains optional references with no count of its own; everything else it said
/// stays.
#[test]
fn old_omni_fill_gains_optional_references() {
    let old = old_fill();
    let store = StoreFile {
        connections: vec![saved_with_fill(old.clone())],
        ..StoreFile::default()
    };
    let read = StoreFile::parse(&store.to_json().unwrap()).unwrap();
    let model = &read.connections[0].models[0];
    assert_eq!(model.id, ModelId::new("nano_banana_2").unwrap());
    let spec = model.spec_for(&old.capability).unwrap();
    assert_eq!(spec.accepts.get(&InputRole::Reference), Some(&u32::MAX));
    assert!(
        !spec.requires.contains(&InputRole::Reference),
        "never required"
    );
    assert_eq!(spec.requires, old.requires);
    assert_eq!(spec.max_outputs, old.max_outputs);
    for role in [InputRole::Source, InputRole::Mask, InputRole::Prompt] {
        assert_eq!(spec.accepts.get(&role), old.accepts.get(&role), "{role:?}");
    }
}

/// A count the saved fill already states for references is kept, even when it differs.
#[test]
fn a_saved_reference_count_is_kept() {
    let saved = old_fill().accepting(InputRole::Reference, 1);
    let store = StoreFile {
        connections: vec![saved_with_fill(saved.clone())],
        ..StoreFile::default()
    };
    let read = StoreFile::parse(&store.to_json().unwrap()).unwrap();
    let spec = read.connections[0].models[0]
        .spec_for(&saved.capability)
        .unwrap();
    assert_eq!(spec.accepts.get(&InputRole::Reference), Some(&1));
}

/// Refresh adds only optional inputs: a saved fill that never mentioned the mask does not gain it,
/// because the mask is required and adding it would change what the saved fill asks for.
#[test]
fn a_required_input_is_never_added() {
    let saved = CapabilitySpec::new(CapabilityId::parse("image.inpaint").unwrap())
        .accepting(InputRole::Source, 1)
        .requiring(InputRole::Source);
    let store = StoreFile {
        connections: vec![saved_with_fill(saved.clone())],
        ..StoreFile::default()
    };
    let read = StoreFile::parse(&store.to_json().unwrap()).unwrap();
    let spec = read.connections[0].models[0]
        .spec_for(&saved.capability)
        .unwrap();
    assert!(!spec.accepts.contains_key(&InputRole::Mask));
    assert_eq!(spec.accepts.get(&InputRole::Reference), Some(&u32::MAX));
}
