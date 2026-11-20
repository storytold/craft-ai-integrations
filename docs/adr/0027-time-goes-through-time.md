# ADR 0027: Time goes through `time`

- **Status:** accepted 20/11/2026
- **Source:** RUSTSEC-2026-0543 (`chrono` unmaintained); replaces [0012](0012-time-goes-through-chrono.md)

## Context

`chrono` is flagged unmaintained with no safe upgrade, so `cargo deny` fails every pull request. `time` is already in the lockfile and is maintained.

## Decision

All date and time conversion goes through the `time` workspace dependency, std-only with `large-dates`, so `craft-ai-core` still compiles for wasm and years past 9999 still convert. A crate owns only its output format string.

## Consequences

No crate hand-rolls calendar or leap-year maths. CONTRIBUTING.md states the rule.
