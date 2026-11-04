# Craft AI MCP Bridge (`craft-ai-mcp`)

A small Rust binary that exposes the Craft AI tool catalogue as **MCP tools** so that any
MCP-capable host — LM Studio, Bionic, or a future client — can call every built-in feature
(`generate-image`, `remove-background`, `generative-fill`, …) through one shared endpoint.

It is the "bridge" between the Craft AI apps and an LLM agent: the agent sees plain MCP tools;
the bridge routes each call into the same `AiClient` job path every app uses (status → build →
plan → consent → start → wait).

## Why it exists

- The 18 Craft AI apps each ship their own GUI, but an LLM host has no way to drive them.
- This crate gives hosts a **single, stable surface** (`/mcp`) that lists every built-in tool
  with a JSON Schema and routes calls through the shared client — so a new app or tool added to
  the shared catalogue is exposed automatically, without touching any host config by hand.

## Architecture (module responsibilities)

| Module | Responsibility |
|---|---|
| `main.rs` | Entry point. Reads `CRAFT_MCP_PORT` (default **12345**) and `CRAFT_MCP_CONNECTIONS`, builds the shared store + in-memory secret store, constructs an `AiClient` for app id `mcp`, binds the listener and runs it. |
| `server.rs` | Raw-TCP streamable-HTTP transport: one thread per connection, reads a single request (headers + body), dispatches to `McpState::handle`, writes a plain-JSON response with `Connection: close`. No HTTP framework on purpose — keeps the binary tiny and dependency-free. |
| `mcp.rs` | Protocol layer only. Maps MCP `initialize` / `tools/list` / `ping` / `tools/call` onto the shared catalogue and the router. Knows nothing about sockets or HTTP. Builds JSON Schemas for tool inputs from each feature's declared inputs. |
| `router.rs` | The heart of the bridge. Turns one MCP `tools/call` into a Craft AI job: looks up the feature, checks `AiClient::status` (the plain-language "what is missing" signal), builds the request from JSON args, plans it, honors consent, then starts and waits on the job. Returns `Succeeded` / `Unavailable` / `NeedsConsent` / `Failed`. |
| `catalogue.rs` | Normalizes the shared `craft_ai_features::builtin()` catalogue into flat `ToolDescriptor`s (id, title, summary, capability, per-app status) that the MCP server lists. |
| `discovery.rs` | Watches the `storytold` org's public GitHub repos to surface apps that have landed but are not yet in the shared catalogue — so a new app shows up without editing this crate. Bounded reads (4 MiB), archived repos excluded, name must contain "craft". |
| `installer.rs` | Two installers: (1) merges this bridge's entry into Bionic's `ng-mcp.json` (idempotent, preserves other servers); (2) generates one `SKILL.md` folder per built-in feature for installation through Bionic's own `skill.install` tool. |

## Running the bridge

```sh
# default: listens on http://127.0.0.1:12345/mcp
./target/debug/craft-ai-mcp

# custom port + connections file location
CRAFT_MCP_PORT=8901 CRAFT_MCP_CONNECTIONS=/path/to/connections.json ./craft-ai-mcp
```

- **Localhost only** — the listener binds `127.0.0.1`, so it is a trusted local host; no remote exposure.
- **Secrets are in-memory (v1).** A real keychain is not wired up yet, so any tool that would send data to a remote destination reports *what is missing* ("prompt to install") instead of crashing. This is intentional and tested.

## Connecting from Bionic / LM Studio

There are two complementary ways the apps reach a host:

### 1. The unified bridge (one entry, all built-in tools)

A single `streamableHttp` server in `ng-mcp.json`:

```json
{
  "id": "c55a52b4-6eca-44ac-926c-f67cceac9f1f",
  "name": "Craft AI",
  "description": "Craft AI image and video tools exposed as MCP.",
  "enabled": true,
  "connection": {
    "type": "streamableHttp",
    "url": "http://127.0.0.1:12345/mcp",
    "headers": {},
    "auth": { "type": "none" }
  }
}
```

`installer.rs::install_mcp_server` writes/updates exactly this entry (idempotent by `SERVER_ID`).
Bionic's schema requires both `headers` and `auth` on every streamableHttp connection; omitting
either rejects the whole file.

### 2. Per-app stdio CLIs (one entry per installed app)

Each Craft AI app ships a headless CLI with an `mcp` subcommand that speaks MCP over **stdio**.
Register each installed one in `ng-mcp.json`:

```json
{
  "id": "<uuid>",
  "name": "WordCraft",
  "description": "Document editing tools from Craft AI.",
  "enabled": true,
  "connection": {
    "type": "stdio",
    "command": "C:\\Programs\\WordCraft\\wordcraft-cli.exe",
    "args": ["mcp"],
    "env": {}
  }
}
```

The installed set (Windows paths; adjust per platform):

| App | stdio command |
|---|---|
| CADCraft | `C:\Programs\CADCraft\cadcraft-cli.exe mcp` |
| Craft Anything | `C:\Programs\Craft Anything\craft-anything-cli.exe mcp` |
| DeckCraft | `C:\Programs\DeckCraft\deckcraft-cli.exe mcp` |
| DesignCraft | `C:\Programs\DesignCraft\designcraft-cli.exe mcp` |
| EffectCraft | `C:\Programs\EffectCraft\effectcraft-cli.exe mcp` |
| FigCraft | `C:\Programs\FigCraft\figcraft-cli.exe mcp` |
| FilmCraft | `C:\Programs\FilmCraft\filmcraft-cli.exe mcp` |
| GridCraft | `C:\Programs\GridCraft\gridcraft-cli.exe mcp` |
| LightCraft | `C:\Programs\LightCraft\lightcraft-cli.exe mcp` |
| PdfCraft | `C:\Programs\PdfCraft\pdfcraft-cli.exe mcp` |
| PhotoCraft | `C:\Programs\PhotoCraft\photocraft-cli.exe mcp` |
| SolidCraft | `C:\Programs\SolidCraft\solidcraft-cli.exe mcp` |
| SoundCraft | `C:\Programs\SoundCraft\soundcraft-cli.exe mcp` |
| TextureCraft | `C:\Programs\TextureCraft\texturecraft-cli.exe mcp` |
| VectorCraft | `C:\Programs\VectorCraft\vectorcraft-cli.exe mcp` |
| WordCraft | `C:\Programs\WordCraft\wordcraft-cli.exe mcp` |
| MailCraft (beta) | `C:\Program Files\MailCraft\mailcraft-cli.exe mcp` |
| MaterialCraft | `C:\Programs\MaterialCraft\materialcraft-cli.exe mcp` |

To add a new app: drop its entry into `ng-mcp.json` using the same shape. No code change is needed
for the bridge itself — it lists whatever is in the shared catalogue, and discovery flags apps that
are on GitHub but not yet catalogued.

## Dynamic discovery of new apps

`discovery.rs::fetch_org_repos("storytold")` pulls the org's public repos from the GitHub API and
`craft_apps()` keeps those whose name contains "craft" (case-insensitive), excluding archived ones.
`unknown_apps(catalogue, org)` returns apps present in the org but absent from the shared catalogue —
the "prompt to add them" signal. This is read-only; it never writes to GitHub.

## Skill generation

`installer.rs::write_skills(base_dir, features)` writes one folder per built-in feature:
`<base_dir>/craft-ai-<feature-id>/SKILL.md`. The front matter (`name`, `display-name`,
`description`, `user-invocable`) follows Bionic's skill schema; the body tells the agent to use the
`craft-ai-mcp` MCP server's `<feature-id>` tool and lists the JSON arguments it accepts.

**Install through Bionic's own `skill.install` tool** (it validates and asks for approval). Do not copy
folders into the global skills directory by hand — see the skill-management guidance.

## Testing

- **Unit / integration tests:** `cargo test -p craft-ai-mcp --lib` (and the crate-level tests in
  `tests/`). They use a client with no saved connections and an empty key store, so every remote
  tool reports "what is missing" — exactly the path we want to exercise. No real provider is contacted.
- **Manual stdio smoke test** (one app):

  ```sh
  printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}\n' \
    | 'C:\Programs\WordCraft\wordcraft-cli.exe' mcp
  ```

  A healthy server replies with a `result` containing `protocolVersion`, `capabilities.tools`, and
  `serverInfo`. Then send `tools/list` (returns every built-in tool) and, if a model is bound,
  `tools/call`.

## Known issues & limitations

- **Four CLIs reject the initialize handshake with newer protocol revisions.** As of this writing,
  `FilmCraft`, `PhotoCraft`, `TextureCraft`, and `MaterialCraft` use an MCP server that (a) returns
  JSON-RPC error `-32602` ("request _meta is missing or has malformed required fields") when the
  initialize request lacks `_meta`, but then (b) returns `-32601` ("initialize" / method not found)
  when `_meta` *is* present inside `params`. The two requirements are mutually unsatisfiable, so a
  standard MCP client cannot complete initialize against them. This is a bug in those CLIs' recent
  updates (they moved to a revision that requires `_meta` but broke the stdio initialize path).
  **Action:** report to the Craft AI team; until fixed, these four will not connect through Bionic's
  stdio MCP. The other 14 stdio CLIs and the unified HTTP bridge are unaffected.
- **Protocol version negotiation.** The unified bridge advertises `2025-03-07`. Newer servers that
  require `_meta` (revisions `2025-11-25` / `2026-07-28`) need the client to carry
  `io.modelcontextprotocol/protocolVersion` and `io.modelcontextprotocol/clientCapabilities` in
  `_meta` *inside* `params`. Bionic's client must be checked against this; see the issue above.
- **Secrets are in-memory (v1).** No real keychain yet; remote destinations report "what is missing"
  rather than failing silently.
- **Click/reference inputs not expressible.** App-specific inputs (e.g. a selection tool's clicks)
  cannot be passed through plain JSON args in v1; the router rejects them with a clear message.
- **Localhost only.** The bridge binds `127.0.0.1`; it is not intended to be exposed on a network.
- **Bionic rejects the whole `ng-mcp.json` if any single entry is schema-invalid.** It does not skip bad entries; one malformed server (e.g. a `streamableHttp` entry missing its required `auth` block) causes *all* servers to fail to load. Validate each entry against Bionic's expectations before adding it.
- **`ng-mcp.json` must match Bionic's native serialization: no UTF-8 BOM, LF line endings, 2-space indent.** A leading BOM or CRLF/reformatted whitespace makes Bionic reject the entire file (zero servers load). Regenerate with `json.dumps(obj, indent=2)` written as LF to be safe.
- **`streamableHttp` entries require an `auth` field (`type: "bearerToken"`).** The local ArtCraft unified bridge currently has no auth and is therefore not loadable by Bionic; it needs a valid (or placeholder) bearer-token block — and its `http://` localhost URL may also need review — before it can be added as a 20th entry.

## Security notes

- Consent is honored exactly as in the apps: local connections never prompt; remote destinations need
  an agreement (or auto-grant when `auto_consent` is true, which is how this bridge runs for trusted
  local hosts). The router surfaces `NeedsConsent` with a disclosure naming *kinds and sizes* of data
  that would leave the machine — never contents.
- Discovery reads only public GitHub metadata; it sends no keys or secrets.
