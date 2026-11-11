# ADR 0021: A cost estimate is shown before consent, and never guessed

- **Status:** accepted 09/11/2026
- **Source:** [#168](https://github.com/storytold/craft-ai-integrations/pull/168)

## Context

A person should see what a hosted job may cost before agreeing to send it. A wrong figure is worse than none.

## Decision

`AiClient::plan` works out a `CostEstimate` from the saved model's `pricing`, reading no key and contacting nothing. The rule in `craft_ai_core::cost::estimate` is the per-image price times the results asked for. A token price, a missing price or an overflow gives `None`.

## Consequences

Models whose publishers price by resolution tier or by token stay unpriced rather than carry an invented figure.
