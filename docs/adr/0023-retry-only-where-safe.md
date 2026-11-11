# ADR 0023: Retry only where a repeat cannot charge twice

- **Status:** accepted 10/11/2026
- **Source:** [#170](https://github.com/storytold/craft-ai-integrations/pull/170)

## Context

Hosted calls fail for passing reasons (rate limits, dropped connections). Retrying a send that the provider already accepted could run and bill a job twice.

## Decision

One retry rule, `craft_ai_core::Backoff`, serves media copies, job sending and polling. `AiClient::start` sends again only after a rate limit, because `Provider::submit` promises that `RateLimited` means nothing was accepted. A dropped send or a server error while sending is never repeated. `Job::wait` retries a failed poll that can pass, within five failures in a row or one minute, and the overall timeout still wins.

## Consequences

Every adapter must keep the `RateLimited` promise. `AiClient::with_retries` sets both rules, and `Backoff::NEVER` turns either off.
