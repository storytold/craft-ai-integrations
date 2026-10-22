# Declare a new tool

Use this page when the suite has no tool for what you want. Examples: "describe a photograph",
"explain this CAD part", "clean up this audio". You declare the tool once. After that, any
model that can do the job can be chosen for it, and every app that declares the same tool
shares the person's choice.

You do not need to edit this repository, the launcher or anyone else's app.

Every Rust block here is a real test (`cargo test -p craft-ai-client --test adopting_tool_guides`).
`--test adopting_drift` fails if a block differs from the tested code.

## What a tool is

A tool is a **declaration, not code**. It is a `Role`:

| Field | What it is | Rules |
|---|---|---|
| `id` | The tool's name across the whole suite, for example `describe-photo`. | Letters, digits and `- _ . : /`. Up to 128 bytes. Agree it with the other maintainers. |
| `label` | What a person sees. | Plain words. Each app may translate it. |
| `capability` | The one thing a model must be able to do. | `<modality>.<task>`, such as `image.describe`. |
| `inputs` | What your tool sends. | Any of `prompt`, `system`, `source`, `mask`, `reference`, `context`. |

Two rules decide which models a person can choose for your tool:

1. The model must offer your **capability**.
2. The model must **accept every input you list**, and must **require none you cannot supply**.

So `inputs` is a promise about what your tool sends. List only what you really send. A model
that needs a mask is not offered for a tool that never sends one.

## Choose the capability

A capability id is `<modality>.<task>`. The set is open, so `cad.explain` is valid with no
library change. Before you invent one, look at the list the suite already shares, so models and apps agree
on spelling:

<!-- snippet: well_known -->
```rust
use craft_ai_core::capability::well_known;

let known: Vec<String> = well_known().iter().map(|c| c.to_string()).collect();
assert!(known.contains(&"image.inpaint".to_string()));
assert!(known.contains(&"image.segment".to_string()));
```

Modalities are `text`, `image`, `video`, `audio`, `document`, `tabular`, `vector` and `mesh`. A
new modality is allowed. Tasks are lower-case tokens with no dots.

Choose the narrowest honest capability. `image.describe` is better than `image.analyse`: a
person looking for a model wants to know it can do exactly this.

## Declare it and choose a model

You do this once, from any process that shares the connections file: the launcher, your app's
Preferences, or a setup script. 

<!-- snippet: declare_and_choose -->
```rust
use craft_ai_core::{
    CapabilityId, CapabilitySpec, ConnectionId, InputRole, ModelId, ModelInfo,
};
use craft_ai_core::{Role, RoleId};

// `launcher` is a `ToolDesk`, and `shared` is the connections file. See the tests for the scene.
// 1. The declaration.
let describe = Role {
    id: RoleId::new("describe-photo")?,
    label: "Describe photograph".into(),
    capability: CapabilityId::parse("image.describe")?,
    inputs: vec![InputRole::Source],
};
launcher.register_tool(describe.clone())?;

// 2. A model that can do it, offered by a connection the person already has.
let vision = ModelInfo::new(ModelId::new("vision-1")?, "Vision 1").with(
    CapabilitySpec::new(describe.capability.clone())
        .accepting(InputRole::Source, 1)
        .requiring(InputRole::Source),
);
let service = serde_json::from_value(serde_json::json!({
    "id": "vision-service", "label": "Vision (on this computer)", "provider": "vision",
    "locality": "local", "auth": {"kind": "none"}, "billing": "free",
    "models": [vision.clone()],
}))?;
shared.update(|store| store.connections.push(service))?;

// 3. The suite-wide choice. It is checked before anything is written.
launcher.choose(&describe, &ConnectionId::new("vision-service")?, &vision.id)?;
assert_eq!(launcher.custom_tools()?.len(), 2); // "tighten-text" from the scene, and this one
```

`register_tool` refuses a name that a built-in tool already uses and a name that is already
taken. `choose` refuses, and writes nothing, when the model cannot do the job.

Declaring is optional for a tool that only your app uses: any app can build a `Role` in code and
call `status` and `plan`. Register it when more than one app should share the choice, because
registering is what protects the contract (next section).

## An id is a contract

Once a tool is registered, its id means one capability with one set of inputs, everywhere.
An app may change the label. It may **not** change what the id means.

<!-- snippet: id_is_a_contract -->
```rust
use craft_ai_client::{AiClient, RoleStatus};
use craft_ai_core::{AppId, CapabilityId, InputRole, Role, RoleId};

let honest = Role {
    id: RoleId::new("shorten")?,
    label: "Shorten".into(),
    capability: CapabilityId::parse("text.complete")?,
    inputs: vec![InputRole::Prompt],
};
launcher.register_tool(honest.clone())?;

// Another app reuses the id for something else. It is refused, not quietly accepted.
let imposter = Role {
    capability: CapabilityId::parse("image.generate")?,
    ..honest
};
let app = AiClient::new(AppId::new("wordcraft")?, shared.clone(), keys.clone());
match app.status(&imposter) {
    RoleStatus::Unavailable { reason } => assert!(reason.contains("does not match")),
    RoleStatus::Ready(_) => panic!("a changed contract must not resolve"),
}
```

The same check runs in `choices`, `picker`, `bind`, `plan` and `choose`. If you need a different
tool, use a different id.

## Send the right data

Build the request from the inputs you declared. Use `Payload::text` for words,
`Payload::inline` for bytes you hold, and `Payload::handle` for a file too big to copy.

<!-- snippet: describe_request -->
```rust
use craft_ai_core::{CapabilityId, InputRole, Payload, Request};

let request = Request::new(CapabilityId::parse("image.describe")?).with_input(
    InputRole::Source,
    Payload::inline("image/png", vec![0x89, b'P', b'N', b'G'])?,
);
assert_eq!(request.count(&InputRole::Source), 1);
```

Answers come back as `Output` events. Choose the shape once and write it down for the next
person: plain text for words, an image or audio payload for media, and `application/json`
for structured results.

If many models and many apps will do the same job, give it a fixed contract like selection has
(`craft_ai_core::segment`): one request shape, one result shape, and checks on everything read
back. That way a click, a box and "nothing there" mean the same on every route.

## Let people set it up

A declared tool shows up in the launcher's tool list and in the shared picker. To make it
easy to run on a computer, add a catalogue entry in `craft-ai-features`: model files with size,
SHA-256, source and licence, and the engines that can run them. That is a pull request to this
repository. It is not needed for the tool to work. A person can always add a connection and
model by hand.

Licences are **disclosure, not clearance**. List every part a recipe runs, with the terms its
publisher states and a link. Say plainly that nothing was checked and that it is not legal advice.

## Remove it

`ToolDesk::remove_tool(&id)` deletes the declaration and every suite and per-app choice that
named it. Built-in tools cannot be removed.

## What a declaration does not do

- It does not install code. A tool is only runnable when some app ships an adapter for the
  chosen connection's provider, and the app that does not says so plainly.
- It does not add a menu, a button or a panel. The interface is the app's.
- It does not pick a model for the person. A model is chosen only when they agree.

Next: [Bring a service or model](bring-a-service.md) to make something that can run it.
