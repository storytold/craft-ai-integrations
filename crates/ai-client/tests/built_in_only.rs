//! A tool whose only local model is built in: Remove background on InSPyReNet, read by the Burn
//! engine. The launcher registers the built-in route, never a ComfyUI one, and engine advice does
//! not offer ComfyUI for it.
use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory, ToolDesk};
use craft_ai_core::{AiError, AppId, ErrorKind, StoreFile};
use craft_ai_features::{Feature, Gpu, GpuVendor, Os, SystemProfile};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

fn remove_background() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "remove-background")
        .unwrap()
}

fn windows_nvidia() -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "RTX 3090 Ti".to_owned(),
            vram_mb: Some(24_576),
        }],
        comfyui_found: true,
    }
}

struct World {
    dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { dir, shared }
    }
    fn client(&self, factory: ProviderFactory) -> AiClient {
        AiClient::new(
            AppId::new("photocraft").unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
        .with_factory(factory)
    }
    fn desk(&self) -> ToolDesk {
        ToolDesk::new(
            self.client(ProviderFactory::standard()),
            self.shared.clone(),
            None,
        )
    }
}

/// A factory with no built-in engine, whatever cargo features are on.
fn without_engines() -> ProviderFactory {
    let mut factory = ProviderFactory::standard();
    for engine in ["rten", "candle", "burn"] {
        factory.unregister(engine);
    }
    factory
}

/// The plain built-in call registers InSPyReNet on Burn with the file it was given and binds the
/// tool; asking for a ComfyUI model is refused and writes nothing.
#[test]
fn the_built_in_route_is_registered_and_comfyui_is_refused() {
    let w = World::new();
    let refused = w
        .desk()
        .enable_local(&remove_background(), "http://127.0.0.1:8188")
        .err()
        .unwrap();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    let refused = w
        .desk()
        .enable_local_recipe(
            &remove_background(),
            "inspyrenet-matte",
            "http://127.0.0.1:8188",
        )
        .err()
        .unwrap();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert!(w.shared.load().unwrap().connections.is_empty());

    let file = w.dir.path().join("inspyrenet.safetensors");
    std::fs::write(&file, b"not really weights").unwrap();
    let done = w.desk().enable_native(&remove_background(), &file).unwrap();
    assert!(done.bound);
    assert_eq!(done.connection.as_str(), "local-remove-background-burn");
    assert_eq!(done.model.as_str(), "inspyrenet");
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 1);
    assert_eq!(store.connections[0].provider, "burn");
}

/// Advice lists Burn alone: never ComfyUI, which has no recipe for this tool. An app with the
/// engine on a graphics card it vouches for is advised it.
#[test]
fn engine_advice_offers_burn_and_never_comfyui() {
    let w = World::new();
    let advice = w
        .client(without_engines())
        .engine_advice(&remove_background(), &windows_nvidia());
    let providers: Vec<&str> = advice.iter().map(|a| a.provider.as_str()).collect();
    assert_eq!(providers, ["burn"]);
    assert!(!advice[0].usable && !advice[0].recommended);

    let mut factory = without_engines();
    factory.register("burn", |_, _| {
        Err(AiError::unsupported("advice never builds a provider"))
    });
    factory.declare_accelerators("burn", &[GpuVendor::Nvidia]);
    let advice = w
        .client(factory)
        .engine_advice(&remove_background(), &windows_nvidia());
    assert_eq!(advice.len(), 1);
    assert!(advice[0].usable && advice[0].recommended, "{advice:?}");
}
