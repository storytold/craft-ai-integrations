# ADR 0004: One TLS setting for every network call

- **Status:** accepted 14/10/2026
- **Source:** [#23](https://github.com/storytold/craft-ai-integrations/pull/23)

## Context

Many adapters and downloaders make network calls. If each built its own HTTP agent, one could weaken certificate checks without anyone noticing.

## Decision

`craft-ai-http` holds the only HTTP agent configuration, using the platform's certificate check. Every adapter and downloader builds its agent through it. The test `crates/ai-http/tests/every_agent_uses_it.rs` reads every crate's source and fails when a `ureq::Agent::config_builder()` call is not followed by `craft-ai-http`'s TLS setting and proxy.

## Consequences

Certificate handling is identical across providers. A new adapter must use `craft-ai-http`. The test checks only that builder call, so review must still catch an agent built some other way.
