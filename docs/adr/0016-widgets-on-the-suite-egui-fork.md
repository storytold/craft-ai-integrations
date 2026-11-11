# ADR 0016: Shared widgets use the suite's egui 0.37 fork

- **Status:** accepted 25/10/2026
- **Source:** [#105](https://github.com/storytold/craft-ai-integrations/pull/105)

## Context

The shared widgets used registry egui 0.36 while the suite had chosen a pinned fork. Two egui versions in one app would not interoperate.

## Decision

`craft-ai-egui` and its consumer example pin `storytold/egui` at `6d518fc7` and `storytold/winit` at `a517b384`. The dependency policy allows only those two fork repositories as git sources. Linux CI installs a software Vulkan renderer for headless snapshots.

## Consequences

An egui app that adopts the widgets repeats the same `[patch.crates-io]` recipe in its own root. Moving to a new fork revision is one change across both manifests.
