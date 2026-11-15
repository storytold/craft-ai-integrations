# Layout: what lives where

This repository is the runtime half of the shared AI layer for the Craft apps: one connection, set up once, used by
every app that has the tool. It owns connections, models, the tool to model binding, the picker
data, consent and jobs. It does not own any app's interface. Each app decides how a tool looks
(a prompt bar, a one-click button, a click on the image), in its own repository.

## In this repository

Crates sit in four folders under `crates/`, by what they reach:

| Folder | Crates | Owns |
|---|---|---|
| `foundation/` | `ai-core`, `ai-http`, `ai-features`, `ai-native`, `ai-media` | The data model and provider trait (wasm-safe), the one TLS and proxy setting every call uses (with a test that fails if any crate builds an agent without it), the tool catalogue and setup options, secret stores and the shared store, and durable copies of provider media. Hosted and local crates both depend on these. |
| `api/` | `ai-openai`, `ai-gemini`, `ai-vertex`, `ai-bfl`, `ai-seedance`, `ai-artcraft`, `ai-declarative` | Hosted services that take a key and may cost money. One crate per provider API shape, plus the runner for a provider described by a data file. Each is registered in `ai-client`'s `ProviderFactory`. |
| `local/` | `ai-comfyui`, `ai-comfy-env`, `ai-sam3`, `ai-engine`, `ai-engine-candle`, `ai-engine-rten`, `ai-engine-burn`, `ai-burn-setup` | Models run on this computer. Local servers reached over loopback (ComfyUI, finding and filling an existing ComfyUI install, SAM 3), and built-in engines: the runner contract, registry and shared provider (`ai-engine`), one crate per engine behind `ai-client`'s `candle`, `rten` and `burn` features, and `ai-burn-setup`, which prepares the saved weights Burn reads. A new model or engine never touches an app. |
| `app/` | `ai-client`, `ai-egui`, `ai-agents`, `ai-mcp` | What an app or agent calls: the app-facing client and tool desk, theme-neutral egui 0.37 widgets (see the consumer recipe below), the switch that lets AI agents use the Craft apps, and the stdio MCP gateway to the apps' own tools (no AI layer dependencies). |

A new crate goes in the folder that matches what it reaches. A crate that calls a hosted service
is `api/`, even when it also has local parts. Package names do not include the folder, so a
consumer pinned to a git revision does not change when a crate moves. A path dependency does.

`docs/design/` holds design notes. `docs/adr/` records each decision that shapes the layer and
why, one file each. `docs/STATUS.md` says what is tested and what is open. `docs/evidence/` keeps
the run records of real runs. The deliberate-bug scripts are in `tools/mutation/`; everything a
script saves goes through `tools/mutation/scrub.py`.

## Not in this repository

| What | Where it goes |
|---|---|
| PhotoCraft's Select subject panel, Generate bar, `ai_cmds`, hover session | `storytold/photocraft`, through its own PR and its maintainers |
| The launcher's Set up AI window, History page and ports | `storytold/craft-launcher` |
| LightCraft's, EffectCraft's and any other app's adapter | That app's repository |
| Anything that draws a toolbar or prompt bar | The app. The shared crates never do. |

The shared code knows an app only as an `AppId` string and as data in the catalogue's `uses`
list. If a change here needs to name an app's behaviour, it belongs in the app.

## How an app uses this

1. Declare a role with the tool's id (the catalogue's `Feature::role()`), for example
   `select-subject`. Do not invent an id of your own.
2. Call `AiClient::status` to grey the tool out, or to say that a model is needed. Never report a
   tool with no model as broken.
3. Depend on a pinned git revision (a tag on `main`), not a path. Path dependencies are for local
   work only and must go before an app PR.
4. A person's choice made for the whole suite is `ToolDesk::choose` (the default every app reads).
   `AiClient::bind` is that one app's own override.

## Using the shared egui widgets

`craft-ai-egui` is optional and has no app adopter in this repository. An egui app repeats the
complete `[patch.crates-io]` recipe in its own root; this repository's patch is not inherited.
The reviewed graph is `storytold/egui` at `6d518fc799681571963842f95b55a6b68431b15a` and
`storytold/winit` at `a517b3846ee8457d84ee4975110bfda6797c44dc`. Keep the library at
`default-features = false` for fonts and let the app choose its font provider (or eframe's
bundled fonts). Do not combine this workspace's `craft-ai-core` with the separate copy in
`craft-libs`; the [consumer smoke](../consumer-smoke/README.md) shows the one-identity check and
passes its locked check and one draw test. That headless result is not native GUI acceptance.

## Branches

- `main` is a state that builds and passes its tests. Apps pin tags on it.
- Work happens on short-lived branches off `main` and returns by pull request. A merged branch is
  deleted, local and remote.
- Review that matters happens at the boundaries: the shared contract in `craft-libs`, and each app
  in its own repository.

## Rules that apply to every change

Test first. A deliberate-bug check for safety rules, with no survivor ignored. Cargo targeted with
`-p`, one process at a time, `-j 8`; no workspace-wide builds. Files under 500 lines. Nothing runs
against a real provider, download or ComfyUI without an explicit go. Say plainly what was run and
what was not.
