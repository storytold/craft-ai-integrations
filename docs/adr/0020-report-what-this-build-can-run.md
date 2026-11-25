# ADR 0020: Report what this build can run apart from what is saved

- **Status:** accepted 05/11/2026
- **Source:** [#158](https://github.com/storytold/craft-ai-integrations/pull/158)

## Context

A saved model could read as Ready while its adapter refused it before sending anything, so a person could agree to send data for a job that could never start.

## Decision

`ProviderFactory::declare_support` lets an adapter state its own limits without a key, a provider or a network call. An adapter that states none is `RouteSupport::Untested`, never `Supported`, and no paid job is used as a probe. `AiClient::route_facts` keeps the saved choice, what this build can run and the last real success as separate facts.

## Consequences

A known refused pairing is refused at `status` and `plan`, before consent, and is not offered. The chosen model stays as it is, with no silent fallback.
