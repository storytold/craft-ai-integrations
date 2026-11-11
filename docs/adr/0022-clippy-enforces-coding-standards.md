# ADR 0022: Clippy enforces the coding standards

- **Status:** accepted 09/11/2026
- **Source:** [#111](https://github.com/storytold/craft-ai-integrations/pull/111)

## Context

Three conventions were not enforced consistently: understandable control flow, short parameter lists and explicit imports.

## Decision

The workspace denies `clippy::cognitive_complexity` (threshold 30 in `clippy.toml`), `clippy::too_many_arguments` and `clippy::wildcard_imports`. Every crate inherits the workspace lints with `[lints] workspace = true`.

## Consequences

A function that trips a limit is split rather than allowed. Generated code must satisfy the same rules.
