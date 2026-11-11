# ADR 0024: Encrypted file as the last secret store

- **Status:** accepted 10/11/2026
- **Source:** [#175](https://github.com/storytold/craft-ai-integrations/pull/175)

## Context

A machine with no OS keychain could not save a key through the standard setup.

## Decision

The standard secret chain is environment, then the OS keychain, then an encrypted file at the default location. Without a passphrase the file layer is locked: it reads no key, reports an existing file as locked rather than empty, and asks for a passphrase to save. A failed file check counts as locked, not missing.

## Consequences

A key is never reported absent while it may still be on disk. Hosts ask for the passphrase when `passphrase_need()` says so.
