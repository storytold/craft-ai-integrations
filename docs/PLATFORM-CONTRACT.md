# Suite-wide AI contract for app maintainers

**Status:** the initial contract crate (`craft-ai-core`) has landed in `storytold/craft-libs`. This workspace still carries a local copy with additional consent terms and model folders; its upstream safety fixes are aligned, but the dependency switch has prerequisites in [CORE-SYNC.md](CORE-SYNC.md). No production app adoption is established by the contract merge. See [STATUS.md](STATUS.md) for what is tested and what is open.

## Ownership boundary

| Shared crates | Each app or launcher |
| --- | --- |
| Versioned connection/model/tool declarations, suite defaults and per-app overrides in one shared store | App-specific tool UI, request construction, document changes, display of results |
| Model compatibility, binding resolution, consent and job contract | A suitable registered execution adapter, if the chosen backend is actually supported in that app |
| Shared picker, connection panel and optional reusable widgets | Where/how those widgets appear, and app-specific setup |

A registered tool is a **declaration**, not installed code. A compatible binding is not evidence that a provider can execute the request. Apps must fail explicitly when the selected adapter is absent, not silently substitute another engine. A separate database should be added only for an identified indexing or coordination need; the current versioned store remains authoritative for these settings.

## Declaring and adopting a tool

1. Define a `craft_ai_core::Role` with a stable `RoleId`, a display label, one `CapabilityId`, and a nonempty set of `InputRole`s. Choose a suite-wide ID with the other maintainers. `ToolDesk::register_tool(role)` writes it once to the shared store; `ToolDesk::custom_tools()` reads registered declarations in any process using that same store. Built-in IDs cannot be replaced. A declaration does not add it to the seven built-in catalogue.
2. Add or discover a connection and a model advertising that capability and the required inputs. `ToolDesk::choose(&role, &connection_id, &model_id)` sets the suite default. The shared update verifies the declared tool contract and model compatibility before persisting the binding. An app can use `AiClient::bind` for an explicit per-app override. Removal with `ToolDesk::remove_tool(&role_id)` deletes the declaration and its suite/app bindings together.
3. An app reads the same store and resolves the role with `AiClient::status`, `choices`, `picker`, `plan` and `start` as appropriate. It may localise the label, but **must not reinterpret the same ID's capability or inputs**. Mismatched contracts are rejected during resolution, model listing, picker creation and binding. Apps still own their UI, input conversion and actual execution route.

Concrete API examples and regressions: `crates/ai-client/tests/custom_tools.rs`, `crates/ai-core/tests/custom_tools.rs` and `crates/ai-native/tests/shared_store.rs`. The shared store is `craft_ai_native::shared::SharedStore`; it uses locked atomic updates for registration, binding and removal. Old version-1 JSON files without `custom_tools` still load with an empty list. Secret values belong in the secret store, never in the shared JSON or a tool declaration.

## Optional shared egui widgets

`craft-ai-egui` is a set of theme-neutral widgets, not a required app dependency. No app dependency
is reproduced in this repository's checkouts. An egui app may adopt it, but must pin the same
coherent graph and repeat the root patch recipe:

- `storytold/egui` at `6d518fc799681571963842f95b55a6b68431b15a` for the egui family;
- `storytold/winit` at `a517b3846ee8457d84ee4975110bfda6797c44dc`.

The library keeps `egui` at `default-features = false`. The app supplies fonts, or enables eframe's
bundled `default_fonts`; the shared widgets do not turn on system-font discovery. kittest `wgpu` and
`snapshot` are test-only features. Keep one `craft-ai-core` identity: this workspace's local copy
and `craft-libs/crates/ai-core` are not interchangeable until the [CORE-SYNC](CORE-SYNC.md) work
lands. The [consumer smoke](../consumer-smoke/README.md) passed its locked check and one draw test with a
single `craft-ai-core` identity and the pinned egui family; its rendererless root does not exercise
winit. It is not an app adoption or a native GUI acceptance result.

## Deliberate limits and next gate

The current surface does **not** ship a dynamic adapter registry for arbitrary user code, a general tool executor, app-specific menus, automatic model installation or a complete provider route. Existing native/hosted adapters have separate capability and real-service gates. The executable adoption example the maintainers asked for exists: `crates/ai-client/tests/third_party_adoption.rs`, walked through in [ADOPT-A-TOOL.md](ADOPT-A-TOOL.md). It proves a declared tool, a bound connection and a registered adapter running one request across two independent app clients, with consent per app, cancellation, an app that lacks the adapter (refused at `plan`, no key read), and a store file from before custom tools existed. Not covered: a real store migration (the schema is still version 1, so there is nothing to migrate yet), a real keychain, and a real hosted service. The apps themselves remain outside this shared-platform task.
