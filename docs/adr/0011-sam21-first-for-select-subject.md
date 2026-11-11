# ADR 0011: SAM 2.1 is offered first for Select subject

- **Status:** accepted 21/10/2026
- **Source:** [#64](https://github.com/storytold/craft-ai-integrations/pull/64), [#85](https://github.com/storytold/craft-ai-integrations/pull/85)

## Context

Select subject offered SAM 3 first, whose weights come under the SAM License. SAM 2.1 has Apache-2.0 weights and runs on the built-in engine with no licence to accept.

## Decision

SAM 2.1 is the first recipe for Select subject, so the plain built-in offer is SAM 2.1 on rten. SAM 3 and Candle stay available as their own options. A SAM 3 choice saved by an earlier build keeps pointing at SAM 3.

## Consequences

Nothing is downloaded or bound by the order alone: it is an offer. Tests find a recipe by its name, never by its position.
