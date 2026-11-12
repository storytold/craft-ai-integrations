# ADR 0012: Time goes through `chrono`

- **Status:** accepted 21/10/2026
- **Source:** [#74](https://github.com/storytold/craft-ai-integrations/pull/74)

## Context

Crates carried their own days-to-date calendar code, which is easy to get subtly wrong.

## Decision

All date and time conversion goes through the `chrono` workspace dependency, std-only so `craft-ai-core` still compiles for wasm. A crate owns only its output format string.

## Consequences

No crate hand-rolls calendar or leap-year maths. CONTRIBUTING.md states the rule.
