# craft-ai-http

The one TLS setting every hosted call in Craft AI uses: certificates are checked by the platform's
own verifier, so a machine whose antivirus or company proxy re-signs HTTPS with an installed root
still works. The trade-off (those installed roots are trusted too) is written at the top of
`src/lib.rs`.

```rust
let config = ureq::Agent::config_builder()
    .tls_config(craft_ai_http::tls_config())
    .build();
```

A test fails if any crate in the workspace builds a `ureq` agent without it.

## Reading a JSON reply

A byte cap on a reply does not bound what parsing it costs: a reply of many tiny objects takes
about a hundred times its size as a `serde_json::Value`. `craft_ai_http::json::parse(bytes, budget)`
builds the same `Value`, but refuses the reply once an estimate of its parsed memory passes
`budget`. A reply that is mostly one long string (a base64 picture) costs about its length, so give
a budget a little above the reply's byte cap. The OpenAI, Gemini, Vertex, declarative and ComfyUI
adapters read their JSON replies through it.
