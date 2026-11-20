# Contributing

Thanks for helping with the Craft AI layer. `README.md` says what
the crates are for and `docs/LAYOUT.md` says what lives where. Read both before a larger change.

## Propose an integration

For a vetted built-in model, connection, engine or workflow suggestion, start with
[Propose an AI integration](docs/PROPOSE-AN-INTEGRATION.md). It creates a checked suggestion and an
issue draft without changing anyone's custom setup. The [adoption guides](docs/adopting/README.md)
cover the subsequent implementation.

## Branches and pull requests

Changes reach `main` through pull requests. The repository allows squash and rebase merges, not merge commits, and deletes the branch after merge.

The shared `craft-ai-core` contract is accepted in `craft-libs`, but this workspace still builds against its own copy in `crates/foundation/ai-core`, which is not interchangeable with it yet ([docs/CORE-SYNC.md](docs/CORE-SYNC.md)). Each app reviews its own adoption in its own repository.

## Rules

The shared rules for every Craft repository are in [AGENTS.md](AGENTS.md) and the
[craft-contrib standards](https://github.com/storytold/craft-contrib/tree/main/standards), including
[testing](https://github.com/storytold/craft-contrib/blob/main/standards/testing.md).

## Building and testing

Builds are heavy, so keep them targeted to the crate you changed. CI runs the whole workspace.

```
cargo test -p <crate> -j 8
cargo clippy -p <crate> -j 8
```

Tests that touch the real OS keychain are `#[ignore]`d and run on request, for example
`cargo test -p craft-ai-native -- --ignored keychain_round_trip`. egui widgets need Rust 1.95.

## Licence

By contributing you agree that your work is licensed under MIT or Apache-2.0, at the option of the
user, matching `LICENSE-MIT` and `LICENSE-APACHE`.
