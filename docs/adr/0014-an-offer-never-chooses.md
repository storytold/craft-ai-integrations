# ADR 0014: An offer never chooses a model

- **Status:** accepted 23/10/2026
- **Source:** [#101](https://github.com/storytold/craft-ai-integrations/pull/101)

## Context

Recommending a model for a tool with no choice saved that choice without asking, even though nothing was downloaded.

## Decision

`ToolDesk::auto_bind_offer` makes a recommendation without writing. `accept_recommended` applies it only on an explicit yes, and leaves the tool alone if it got a choice in the meantime. The check and the write happen in one store update.

## Consequences

An app must show the offer and ask first. The old `auto_bind` was removed from the public API.
