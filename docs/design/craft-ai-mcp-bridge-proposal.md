# Proposal: Craft AI MCP Bridge (`craft-ai-mcp`) — consent, auth & shared store

**Status:** Draft for review (not yet implemented)
**Related draft work:** [storytold/craft-libs#248](https://github.com/storytold/craft-libs/issues/248)
**Requested by:** Jeremy8776 (PR #154 review) — "open a proposal issue first"

This doc proposes concrete answers to the three questions raised in PR #154 for the
`craft-ai-mcp` bridge: **how consent reaches a person through the bridge, how a caller is
authenticated, and how it uses the shared store.** It also addresses the fourth concern — that the
startup pattern "looks like malware."

It does **not** change any person's custom models, saved connection, key, binding or consent. A
proposal doc is not an implementation; nothing here ships until reviewed and approved.

## Why this proposal exists

The `craft-ai-mcp` crate exposes the Craft AI tool catalogue as MCP tools so LM Studio / Bionic can
drive them. As reviewed in PR #154, it should not merge as-is because:

1. **Consent is skipped.** `McpState::new` sets `auto_consent: true`, so the router grants consent
   itself for hosted connections. A configured connection is *not* permission to send data.
2. **Any local program can use it.** The port has no authentication; the `Origin` check stops
   browsers but not other processes on the machine. Combined with (1), that is a route to a paid
   service with no prompt.
3. **It is not the shared connection.** It reads a custom-path connections file and an empty
   in-memory secret store, so a person's saved setup does not appear to be used.
4. **The start-up method looks like malware.** A `.bat` that starts `target\debug\craft-ai-mcp.exe`
   minimised plus a logon scheduled task is a launch pattern Craft does not ship and that antivirus
   or a careful reader would flag.

## Current behavior (the gap)

`crates/ai-mcp/src/main.rs` builds the client as:

```rust
let shared  = SharedStore::new(connections.clone()); // custom path (CRAFT_MCP_CONNECTIONS, else home default)
let secrets = Arc::new(MemoryStore::new());          // in-memory, empty — no env, no keychain
let client  = AiClient::new(AppId::new("mcp").unwrap(), shared, secrets);
```

A real app instead uses the canonical construction (`AiClient::standard` / `manage.rs`):

```rust
SharedStore::at_default_location() + LayeredStore([EnvStore, KeychainStore])
```

So the bridge today (a) does not read keys from env or the OS keychain, and (b) grants consent
silently. Both are the gaps this proposal closes.

## Proposed design

### 1. How consent reaches a person through the bridge

**Fail closed by default.** `auto_consent` defaults to `false`. The bridge never grants consent on
its own. Keep `McpState::with_policy(client, auto_consent, timeout)` for tests and an explicit,
clearly-documented opt-in (e.g. `CRAFT_MCP_AUTO_CONSENT=1`) for trusted local development only.

**Surface the disclosure.** When a tool needs consent, return the existing
`ToolCallResult::NeedsConsent { disclosure }`. The `Disclosure` names *kinds and sizes* of data that
would leave the machine — never contents. The agent relays this to the user in conversation.

**Record approval as an MCP action.** Add consent tools so a human's approval is recorded *through*
the bridge rather than bypassed:

- `consent/pending` — list disclosures currently requiring consent (so the agent can show what is outstanding).
- `consent/grant { disclosure_id, [expires_at] }` — grant one specific pending disclosure, persisted
  to the app's per-app `ConsentFile` ledger (`consent/<app>.json`) via `ledger.grant(d)` /
  `grant_until(d, expires_at)`. Same semantics and revocability as the apps' own ledger.
- (Optional) `consent/revoke { disclosure_id }`.

**Why this keeps a human in the loop.** The agent must (a) show the disclosure to the user, (b) get an
explicit "yes", then (c) call `consent/grant`. There is no path where consent is granted without a
human saying yes. The grant is persisted and revocable, matching app ledger semantics.

**Local vs remote.** Preserve the existing rule that purely-local operations (`Locality::Local`) do
not require consent — they never leave the machine. Only remote destinations trigger the consent
round-trip.

### 2. How a caller is authenticated

The trust boundary stays **localhost-only** (bind `127.0.0.1` only, as today). On top of that:

- **Recommended now:** keep localhost-only and rely on fail-closed consent (§1) so even an unauthenticated
  local process cannot send data without explicit per-call consent. This closes the dangerous
  combination (silent consent) without blocking the merge on auth machinery.
- **Follow-up hardening:** once §3's keychain wiring is in place, add an optional per-install bearer
  token: generate a random token at first run, store it in the OS keychain (`KeychainStore`), require
  `Authorization: Bearer <token>` on `/mcp`, and write it into the same `ng-mcp.json` entry's
  `headers` field the installer already writes. A stray local process then needs the keychain-held
  token, not just network reachability.

**Open question:** is localhost-only + fail-closed consent acceptable as day-one, with the bearer
token as a follow-up? Or should the token be required from the start?

### 3. How it uses the shared store

Switch to the canonical app construction:

```rust
let client = AiClient::standard(AppId::new("mcp"))?;
// == SharedStore::at_default_location() + LayeredStore([EnvStore, KeychainStore])
```

This makes the bridge read the user's saved connections from the shared store and keys from env vars
→ OS keychain (Credential Manager / Keychain / Secret Service), degrading gracefully to an encrypted-file
fallback when no keychain exists. Keep `CRAFT_MCP_CONNECTIONS` as an opt-in override for testing/dev,
but default to the canonical location so a person's real setup is used.

Consequence: the bridge can now *see* whether a model/connection is bound; if not, it reports "what is
missing" (the intended prompt-to-install signal) instead of silently failing.

### 4. Start-up / trust boundary ("looks like malware")

Remove the auto-start and minimized-window pattern from this PR:

- **Drop the logon scheduled task** and the `start-craft-ai-mcp.bat` minimized launch. The bridge should
  be started deliberately, not hidden at logon.
- Prefer one of: (a) a normal foreground process the user starts when they want MCP tools; (b) Bionic's
  own lifecycle starting/stopping it, if Bionic supports launching long-running MCP servers; or (c) a
  clearly-named, non-minimized launcher run on purpose.
- **Document the trust boundary explicitly** in README/docs: what the process does, where it listens,
  what data it can access, and how to stop/uninstall it. Transparency is the antidote to "looks like
  malware."

## What this does NOT do

- No paid or live service calls.
- No changes to any user's models, connections, keys, bindings or consent.
- No dynamic adapter registry / arbitrary code execution (a registered tool remains a declaration, not
  installed code).

## Open questions for review

1. **Consent mechanism:** MCP `consent/grant` tools (§1) vs. requiring approval in the app/launcher UI?
   I propose the MCP tools — they fit the host model and keep the human in the loop.
2. **Auth level:** localhost-only + fail-closed consent now, bearer token later — acceptable, or day-one?
3. **Start-up:** remove auto-start entirely, or integrate with Bionic's lifecycle?
4. **Alignment with craft-libs#248:** which parts of that draft should this adopt vs. diverge from?

## Next steps

1. Review this proposal (Jeremy8776 + dylanmtaylor).
2. On approval, implement in a focused PR: fail-closed consent + consent tools; `AiClient::standard`
   wiring; remove the auto-start pattern.
3. Add scripted tests for the consent round-trip and store wiring — no live/paid calls.
