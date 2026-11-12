# ADR 0010: Keep a local `craft-ai-core` until craft-libs has what it lacks

- **Status:** accepted 20/10/2026
- **Source:** [CORE-SYNC.md](../CORE-SYNC.md)

## Context

The contract crate has landed in `storytold/craft-libs`, but it lacks `ConsentTerm` and `model_folders`. Dropping either would break consumers or lose a person's model-folder map on a rewrite.

## Decision

This workspace keeps its own copy of `craft-ai-core`. Safety fixes stay aligned in both copies. The switch to one pinned craft-libs revision waits until the missing additions are reviewed upstream.

## Consequences

Two copies exist for now and must not be mixed in one app. CORE-SYNC.md lists the steps to switch.
