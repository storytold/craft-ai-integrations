# ADR 0006: Local engines share one runner contract

- **Status:** accepted 17/10/2026
- **Source:** [#39](https://github.com/storytold/craft-ai-integrations/pull/39), [ADD-AN-ENGINE.md](../ADD-AN-ENGINE.md)

## Context

Engines that run models on this computer (Candle, Burn and others) each needed job handling, cancel, progress and weight loading. Without a shared contract every app would carry its own copy.

## Decision

`craft-ai-engine` defines `ModelRunner` and `LoadedModel`, and `EngineProvider` is the one `Provider` that runs any registered model. It loads weights once, makes jobs for one model take turns, ends a cancelled job at once and turns a panicking runner into a failed job. `ProviderFactory::register_engines` is the whole of enabling an engine in an app.

## Consequences

A new model or engine never touches an app. Engine authors get the same conformance checks as every adapter through `testing::check_runner`.
