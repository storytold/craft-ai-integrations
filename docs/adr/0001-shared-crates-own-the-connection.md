# ADR 0001: The shared crates own the connection, each app owns its interface

- **Status:** accepted 12/10/2026
- **Source:** [LAYOUT.md](../LAYOUT.md)

## Context

Every Craft app needs the same AI connections, models and consent, but each app presents a tool differently: a prompt bar, a one-click button, a click on the image.

## Decision

This repository owns connections, models, the tool to model binding, picker data, consent and jobs. It never draws a toolbar or prompt bar. Each app decides how a tool looks, converts its own document and stores its own results, in its own repository. The shared code knows an app only as an `AppId` string and catalogue data. Apps depend on a pinned git revision of `main`, never a path.

## Consequences

A change here that needs to name an app's behaviour belongs in that app. Path dependencies are for local work only and must go before an app PR.
