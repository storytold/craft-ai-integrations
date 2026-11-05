# Craft AI integrations

<a href="https://sonarcloud.io/project/overview?id=storytold_craft-ai-integrations"><img alt="Scanned by SonarCloud" src="https://img.shields.io/badge/Scanned_by-SonarCloud-4b9fd5?style=flat-square&logo=sonarqubecloud&logoColor=white"></a>

One AI connection for every Craft app. A person sets up a tool and its model once, in the launcher or
natively in any app, and every app that has that tool uses it with no further setup. The shared crates
own connections, models, the tool to model binding, the model picker, consent and jobs. Each app keeps
its own interface for each tool. Where the suggested models or providers do not suit someone, they can
add their own connection, model or tool route and plug it in.

This workspace is the runtime half of the Craft AI layer: provider adapters, secret stores, the app-facing
client, local runtime setup and reusable egui widgets. The pure contract they all speak
(`craft-ai-core`: connections, capabilities, requests, jobs, consent, the shared store format) has
landed in `storytold/craft-libs`. This workspace still uses a local copy with newer consent-term and
model-folder additions. It carries the upstream safety fixes, but cannot switch to a pinned upstream
revision until the required additions are reviewed there. See [docs/CORE-SYNC.md](docs/CORE-SYNC.md)
for that boundary and the dependency-switch gates. The extension APIs are still evolving.

Layout and rules mirror craft-libs: edition 2024, `MIT OR Apache-2.0`, no `unsafe`, and every
`lib.rs` denies `unwrap`, `expect` and `panic` in shipped code.

## Crates

| Crate | What it is |
|---|---|
| `craft-ai-core` | The data model and provider trait. Serde only, no I/O, no async runtime, compiles for wasm. |
| `craft-ai-native` | Desktop only: secret stores (OS keychain, passphrase-encrypted file, environment), the shared store file with a locked atomic writer, consent and history files. |
| `craft-ai-client` | What an app holds: `AiClient` (status, plan, consent, start, `Job`), `ToolDesk` (which model does each tool), `ConnectionManager`, and the `ProviderFactory` that adapters register into. |
| `craft-ai-features` | Wasm-safe catalogue of AI tools, where each would appear, how each can run on a machine (model files with size, SHA-256, source and licence) and what a machine still needs. |
| `craft-ai-http` | The one TLS setting every hosted call and download uses: the platform's certificate check. |
| `craft-ai-media` | Durable local copies of hosted media links, with the checks that needs. |
| `craft-ai-openai` | OpenAI-compatible text (streaming, tools, structured output) and images (generate, edit, inpaint). |
| `craft-ai-bfl`, `craft-ai-vertex`, `craft-ai-seedance`, `craft-ai-artcraft` | Black Forest Labs FLUX, Vertex express-mode Gemini images, BytePlus ModelArk Seedance video, and the ArtCraft Omni API-key route. |
| `craft-ai-declarative` | A provider described by a small data file instead of Rust. Design: `docs/design/declarative-adapter.md`. |
| `craft-ai-comfyui`, `craft-ai-sam3` | Local ComfyUI workflows, including imported subject-selection graphs (`image.segment`) whose masks are checked before an app sees them; the SAM 3 point and session clients; the older direct SAM 3 detector bridge. |
| `craft-ai-engine`, `craft-ai-engine-candle`, `craft-ai-engine-rten`, `craft-ai-engine-burn` | Built-in model engines: the runner contract and the one provider that runs any registered model on this computer; SAM 3 on Candle (Apache-2.0 only; see its NOTICE); SAM 2.1 on rten, a pure Rust engine that reads the publisher's `.onnx` files as they are, on the processor (the default); and SAM 2.1 on Burn, an optional graphics-card engine that reads weights prepared once on this computer (`craft-ai-burn-setup`). Guides: `docs/ADD-AN-ENGINE.md`, `docs/BURN-ENGINE.md`. |
| `craft-ai-comfy-env` | Finds an existing ComfyUI Desktop install, inventories its model files and node packs, downloads missing model files into it. |
| `craft-ai-egui` | Theme-neutral egui widgets: status, model picker, consent prompt, job progress, connection panel, history. Needs Rust 1.95. |

`craft-ai-egui` targets egui 0.37 and resolves it through the reviewed storytold family at
`6d518fc799681571963842f95b55a6b68431b15a`, with winit at
`a517b3846ee8457d84ee4975110bfda6797c44dc`. An adopting app repeats the complete `[patch.crates-io]`
recipe at its own root; `consumer-smoke/` is the small compile/check root for that recipe and the
single `craft-ai-core` identity. Bundled fonts stay off in the library and are a host choice.

Building on this layer (use a tool in your app, declare a new tool, or bring a service or model): start at [docs/adopting](docs/adopting/README.md). The short version is [docs/ADOPT-A-TOOL.md](docs/ADOPT-A-TOOL.md).

Which crate owns what, and what stays in each app: [docs/LAYOUT.md](docs/LAYOUT.md). The shared-tool
contract and its limits: [docs/PLATFORM-CONTRACT.md](docs/PLATFORM-CONTRACT.md). Custom connections,
model sources and node packs: [docs/custom-connections-and-sources.md](docs/custom-connections-and-sources.md).

## Building and testing

Builds are heavy, so keep them targeted and run one cargo process at a time:

```sh
cargo test -p craft-ai-client -j 8
cargo clippy -p craft-ai-client --all-targets --locked -j 8 -- -D warnings
cargo check -p craft-ai-core --target wasm32-unknown-unknown --locked
cargo fmt --all --check
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy on the whole workspace, the wasm check, the tests on
Linux and Windows, and `cargo-deny`. The test that touches the real OS keychain is `#[ignore]`d:
`cargo test -p craft-ai-native -- --ignored keychain_round_trip`. Deliberate-bug checks live in
[tools/mutation](tools/mutation/README.md).

Examples: `cargo run -p craft-ai-client --example custom_tool`, and
`cargo run -p craft-ai-egui --example picker_demo` (sample data only).

## What has and has not been verified

Read [docs/STATUS.md](docs/STATUS.md) before relying on a provider or tool. In short:

- **Tested against scripted servers on localhost:** every hosted adapter, the media copier, the generic
  ComfyUI adapter, the declarative adapter and the client flow.
- **Run for real on one machine:** the SAM 3 point and session clients through a headless ComfyUI; SAM 2.1 on rten with the published ONNX files; and an imported selection graph on ComfyUI 0.22.3. All on Windows, with one sample picture.
- **Migrated shared widgets, headless:** the targeted `craft-ai-egui`/`craft-ai-engine` tests and the
  locked consumer smoke passed 126 tests with 0 failures and 4 ignored doctests under managed job
  `egui037-ai-final-ci-20261025-1331`, with formatting that left the worktree unchanged, strict
  all-target clippy for those two packages, five committed kittest snapshots, one draw test and one
  `craft-ai-core` identity on Linux. The earlier 108-test mutation checkpoint is historical. The
  rendererless smoke does not use winit.
- **Not verified:** any real hosted service, TLS behind an intercepting proxy, macOS and Linux
  keychains, macOS builds of the test suite, and any app running a tool through the shared binding.
  Native widget behavior on Linux X11/Wayland and Windows (focus, keyboard, clipboard, IME and
  rendering), physical-GPU qualification, performance, and app adoption of the widgets also remain
  unverified.

## Licence

MIT or Apache-2.0, at your option (`LICENSE-MIT`, `LICENSE-APACHE`).
