# Consumer smoke root

This separate `[workspace]` checks the dependency recipe an app copies when it adopts
`craft-ai-egui`. It is **not** a repository workspace member and it changes no root manifest or
root lockfile.

- Path dependencies: `../crates/ai-egui` and the same `../crates/ai-core` package.
- One `craft-ai-core` identity: `src/lib.rs` names `ConnectionDraft`, `ConnectionId` and
  `DraftError` through `craft_ai_core` and takes them from the public widget state. A second core
  copy makes the crate fail to compile.
- Repeated patches: the complete `[patch.crates-io]` recipe is at the reviewed egui commit
  `6d518fc799681571963842f95b55a6b68431b15a` and winit `a517b3846ee8457d84ee4975110bfda6797c44dc`.
- Fonts: the smoke root enables `egui/default_fonts`. The library dependency does not.
- Synthetic data only: the unit test draws the form, progress and empty history with no provider,
  download, shared store or OS keychain call.

The separate `Cargo.lock` pins this smoke root. Run the locked checks and inspect the duplicate
report from
the `consumer-smoke/` directory:

```sh
cargo metadata --locked --format-version 1 > /dev/null
cargo check --locked -p craft-ai-consumer-smoke
cargo test --locked -p craft-ai-consumer-smoke
cargo tree --locked -d
cargo tree --locked -i craft-ai-core
```

Expected graph: exactly one `craft-ai-core` identity and one egui family at the reviewed commit.
This headless smoke does not select winit; the paired winit patch is included for native adopters.
The smoke explicitly clears its unrendered texture delta. A native host must apply texture updates
to its renderer. Do not copy this root's lockfile into an app; each consumer resolves and reviews
its own.
