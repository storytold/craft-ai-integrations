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

### 1. How consent reaches a person — in the launcher, not through the bridge

Jeremy's correction: an MCP `consent/grant` tool would let *whoever calls the bridge* approve their
own request — including the AI model and any local program. That defeats human-in-the-loop consent.
So approval must happen where a **person** acts: in the app or launcher, not through the bridge.

- **Fail closed by default.** `auto_consent` defaults to `false`. The bridge never grants consent on
  its own. Keep `McpState::with_policy(client, auto_consent, timeout)` for tests and an explicit,
  clearly-documented opt-in (e.g. `CRAFT_MCP_AUTO_CONSENT=1`) for trusted local development only.
- **Surface the disclosure.** When a tool needs consent, return the existing
  `ToolCallResult::NeedsConsent { disclosure }`. The `Disclosure` names *kinds and sizes* of data that
  would leave the machine — never contents. The agent relays this to the user in conversation.
- **Approve in the launcher.** The human approves from the ArtCraft Launcher (see §5), which writes
  the grant into the per-app `ConsentFile` ledger (`consent/<app>.json`) via `ledger.grant(d)` /
  `grant_until(d, expires_at)`. The bridge re-checks the ledger on start and proceeds only if a valid
  grant exists. There is no path where consent is granted without a person approving in the launcher/app.
- **Local vs remote.** Preserve the existing rule that purely-local operations (`Locality::Local`) do
  not require consent — they never leave the machine. Only remote destinations trigger the consent
  round-trip.

### 2. How a caller is authenticated — from the start

Jeremy: callers must authenticate *from the start*, not as a later step. So authentication is core,
not an optional hardening:

- **Trust boundary stays localhost-only** (bind `127.0.0.1` only).
- **Per-install bearer token.** Generate a random token at first run, store it in the OS keychain
  (`KeychainStore`), and require `Authorization: Bearer <token>` on `/mcp`. The token is written into
  the same `ng-mcp.json` entry's `headers` field the installer already writes (and surfaced in the
  launcher so a human can manage it). A stray local process then needs the keychain-held token, not
  just network reachability.
- **The AI model / agent is a caller too.** Because the bridge is driven by an LLM host, the host's
  own identity must be authenticated — which is why approval lives in the launcher (a human boundary),
  not in an MCP tool the model could call.

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

### 4. Start-up & registration: user-initiated chat install ("looks like malware")

Replace the auto-start / minimized-window pattern with a **user-initiated, copy-paste install prompt** —
the same shape NVIDIA uses for agent-driven MCP onboarding. The user pastes the prompt into Bionic /
LM Studio; the agent then (a) starts the bridge if it is not already running, and (b) adds/verifies the
`Craft AI` entry in `ng-mcp.json`. Because the human initiated it, there is no hidden logon task and the
consent to run a local process + register an MCP server is explicit.

**Copy-paste prompt** (shipped in the repo README / PR description so users can grab it):

```text
Add a Streamable HTTP MCP server named "Craft AI" at http://127.0.0.1:12345/mcp to my user-level Bionic
MCP configuration (ng-mcp.json). First, if the Craft AI bridge is not already running on that port, start
it from C:\\Programs\\craft-ai-integrations\\target\\debug\\craft-ai-mcp.exe — a local-only process listening
on 127.0.0.1. Then add or verify this entry in ng-mcp.json:
{ "id": "c55a52b4-6eca-44ac-926c-f67cceac9f1f", "name": "Craft AI", "description": "Craft AI image and
video tools exposed as MCP.", "enabled": true, "connection": { "type": "streamableHttp",
"url": "http://127.0.0.1:12345/mcp", "headers": {}, "auth": { "type": "none" } } }
If you cannot configure MCP servers directly, show me where to add it manually. Afterward, restart Bionic
so it reconnects.
```

- **Drop the logon scheduled task** and the `start-craft-ai-mcp.bat` minimized launch from this PR. The
  bridge is only ever started when the user pastes the prompt (or runs the binary on purpose).
- **Document the trust boundary explicitly** in README/docs: what the process does, where it listens
  (`127.0.0.1` only), what data it can access, and how to stop/uninstall it (stop the process / remove
  the `Craft AI` entry from `ng-mcp.json`). Transparency is the antidote to "looks like malware."
- **Verified on this machine:** adding the entry to `~/.lmstudio/apps/bionic/.internal/ng-mcp.json` and
  restarting Bionic makes the Craft AI MCP appear and connect (bridge running, `tools/list` returns all
  built-in tools).

### 5. Product: ArtCraft Launcher control for the bridge

To make consent and service management a *human* act, add a section to the ArtCraft Launcher Settings
(see [artcraft-settings-toggle-area.png](images/artcraft-settings-toggle-area.png)) that controls the
MCP bridge and surfaces app visibility. This is the "approve in the launcher" point Jeremy asked for,
and it also answers "where are my apps / what's installed."

Proposed controls (in the highlighted area of ArtCraft Settings):

- **Enable/disable the Craft AI MCP bridge** — start/stop the `craft-ai-mcp` service and toggle its
  entry in `ng-mcp.json`. This replaces the hidden logon scheduled task with a visible, user-controlled
  switch.
- **Consent approvals** — list outstanding disclosures and let the human approve/revoke them (writing
  to the per-app `ConsentFile` ledger). This is where "a person approves in the launcher" happens.
- **Install location + installed apps** — show the default install folder (`C:\Programs`, see
  [artcraft-install-folder.png](images/artcraft-install-folder.png)) and the list of installed Craft
  apps, so users know what's present and where.
- **MCP registration for other hosts** — an option to (re)write the `Craft AI` entry into a host's MCP
  config (Bionic/LM Studio), and optionally expose the bridge for third-party local clients.

Evidence of the target surface:

- [bionic-mcp-servers-list.png](images/bionic-mcp-servers-list.png) — Bionic's "Connected MCP Servers"
  list, showing all Craft AI apps connected (the goal state the bridge joins).

## What this does NOT do

- No paid or live service calls.
- No changes to any user's models, connections, keys, bindings or consent.
- No dynamic adapter registry / arbitrary code execution (a registered tool remains a declaration, not
  installed code).

## Open questions for review

1. **Consent location:** approve in the ArtCraft Launcher (§5) rather than via an MCP `consent/grant`
   tool — agree? (Jeremy: yes, a person must approve in the app/launcher.)
2. **Auth level:** per-install bearer token required from the start (callers authenticate from the
   beginning), not as a later hardening — agree?
3. **Start-up / registration:** user-initiated chat install (§4) *plus* launcher control (§5) — is
   the combination right, or should the launcher be the sole control surface?
4. **Alignment with craft-libs#248:** which parts of that draft should this adopt vs. diverge from?

## Next steps

1. Review this proposal (Jeremy8776 + dylanmtaylor).
2. On approval, implement in a focused PR: fail-closed consent (approval via launcher); day-one
   bearer-token auth; `AiClient::standard` wiring; remove the auto-start pattern.
3. Add scripted tests for the consent round-trip and store wiring — no live/paid calls.
4. Coordinate the ArtCraft Launcher controls (§5) with the launcher team / craft-libs#248.
