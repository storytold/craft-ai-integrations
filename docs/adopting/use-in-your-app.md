# Use AI tools in your app

This guide takes an app from "no AI" to "a tool that runs, asks first when it must, can be
cancelled, and fails in words a person can read". You write the tool's interface. The shared
layer does the rest: connections, keys, which model does what, consent, jobs and the log.

Every Rust block on this page is a real test. `cargo test -p craft-ai-client --test adopting_app_guide`
runs each one against a scripted stand-in service, and `--test adopting_drift` fails if a block here
differs from the tested code. Nothing touches the network or your real connections.

In the examples, `client` is the client from step 2 and `role` is the tool from step 3. A block
that needs more says so.

**You need:** Rust 1.90 or newer, and a native (desktop) app. `craft-ai-core` also builds for
wasm, but the client, the stores and the adapters do not.

## 1. Add the dependency

The crates are not on crates.io. Pin a revision of `main` and never use a path dependency in
anything you ship:

```toml
[dependencies]
craft-ai-client   = { git = "https://github.com/storytold/craft-ai-integrations", rev = "<a commit on main>" }
craft-ai-core     = { git = "https://github.com/storytold/craft-ai-integrations", rev = "<same commit>" }
craft-ai-features = { git = "https://github.com/storytold/craft-ai-integrations", rev = "<same commit>" }
```

Use one revision for all of them. `craft-ai-core` is also in `storytold/craft-libs`, and the local
copy here is ahead of it (see [CORE-SYNC](../CORE-SYNC.md)), so do not mix the two.

## 2. Make one client

One `AiClient` per app. It knows your app's id, the shared connections file, the key stores and
the adapters your app ships.

<!-- snippet: standard_client -->
```rust
use craft_ai_client::AiClient;
use craft_ai_core::AppId;

// Reads the shared connections file the launcher writes, and keys from the environment
// and then the OS keychain.
let client = AiClient::standard(AppId::new("wordcraft")?)?;
```

Pick your app id once and keep it. It is how consent is scoped and how a person gives your app
its own choice of model. Use the lower-case name that the launcher uses.

Every app works with no connection at all. Your app must start, and its other features must work,
when nobody has set up AI.

`CRAFT_AI_DIR` moves the whole folder (connections, consent, history). Point it at a temporary
folder in your own tests so they never read or change a real person's settings.

## 3. Say which tool you are offering

A tool is a `Role`: an id, a label, one capability, and the inputs the tool sends. The
capability and the inputs decide which models can ever be chosen for it.

For a tool the suite already knows, take the role from the catalogue. Then you inherit the
person's choice and the setup recipes for free:

<!-- snippet: catalogue_role -->
```rust
use craft_ai_features::builtin;

let select_subject = builtin()
    .into_iter()
    .find(|feature| feature.id == "select-subject")
    .and_then(|feature| feature.role())
    .ok_or("the catalogue has no such tool")?;
assert_eq!(select_subject.capability.as_str(), "image.segment");
```

For a tool nobody has declared yet, write the `Role` yourself. That is the subject of
[Declare a new tool](declare-a-tool.md). Do not invent an id for a tool the catalogue already has.

## 4. Ask `status`, and draw what it says

`status` answers one question: can this tool run right now, in this app? It reads no key and
sends nothing.

<!-- snippet: status -->
```rust
use craft_ai_client::RoleStatus;

match client.status(&role) {
    RoleStatus::Ready(ready) => {
        // Enable the tool. You can show which model will do the work.
        assert_eq!(ready.model_label, "Tidy 1");
    }
    RoleStatus::Unavailable { reason } => {
        // Grey the tool out and show `reason` exactly as it is.
        println!("{reason}");
    }
}
```

Draw the three states the same way in every place:

| What you see | What to show |
|---|---|
| `Ready` | The tool, switched on. Optionally the model and whether it is free, metered, local or online. |
| `Unavailable`, nothing chosen | The tool, switched off, with the reason and a way to fix it (see step 9). Never call this an error. |
| `Unavailable`, anything else | The same. The reason names the problem: a missing key, a switched-off connection, a model that cannot take your inputs, or "This app can't run the chosen model yet". |

`reason` is written for a person. Show it as it is. Do not add engine names, hardware words or
developer words to it, and do not translate it into something vaguer.

Call `status` again whenever the tool is about to be used, not once at start-up. A person can
change their choice in the launcher while your app is open.

## 5. Build the request

A `Request` names a capability and carries typed inputs. The type matters: a mask is not just
another picture.

<!-- snippet: request -->
```rust
use craft_ai_core::{InputRole, Payload, Request};

let request = Request::new(role.capability.clone())
    .with_input(InputRole::Prompt, Payload::text("Tighten this paragraph."))
    .with_param("tone", serde_json::json!("plain"));
```

- **Inputs** are `prompt`, `system`, `source`, `mask`, `reference` and `context`. Send only the
  ones your `Role` lists. A model that cannot take them is never offered for your tool.
- **Payloads** are `Payload::text`, `Payload::inline(media_type, bytes)` (up to 32 MiB by
  default) or `Payload::handle(uri, media_type, size)` for something too big to copy.
- **Params** are named values. The consent prompt shows their names, never their values.
- Some tools have a fixed shape that every model and every app agrees on. Selection is one:
  use `craft_ai_core::segment` (`with_clicks`, `with_area`, `read_found`, `mask`) and never invent
  your own parameters or result labels for it.

## 6. Plan

`plan` checks the request against the chosen model and builds the disclosure: what would leave
this computer, and where it would go. It sends nothing and reads no key.

<!-- snippet: plan -->
```rust
let plan = client.plan(&role, request)?;
println!("{} will use {}", role.label, plan.connection_label());
```

`plan` is where most mistakes show up, before anyone is asked anything:

| `ErrorKind` | Meaning | Do |
|---|---|---|
| `Unsupported` | This app cannot run the choice, or the model cannot take that input or that many of it. | Show the message. Offer step 9. |
| `InvalidRequest` | The request is wrong: a missing required input, an oversized payload. | This is your bug, or the person gave bad input. Fix or say so. |

## 7. Consent

A local model needs no consent. Anything that leaves the computer does, and the layer enforces
it. `start` refuses with `ConsentRequired` unless the ledger covers this app, this connection,
this destination, this capability and each kind of data in the plan.

<!-- snippet: consent -->
```rust
use craft_ai_core::{ConsentLedger, ConsentTerm, ErrorKind};

let plan = client.plan(&role, request)?;
let mut saved = ConsentLedger::default(); // in a real app: load it, see below

// Without consent the job does not start, and no key is read.
let refused = client.start(&plan, &saved).expect_err("must refuse");
assert_eq!(refused.kind, ErrorKind::ConsentRequired);

if plan.needs_consent(&saved) {
    let d = plan.disclosure();
    println!("Send {} item(s) to {}?", d.items.len(), d.destination);
    // The person answers with one of ConsentTerm::ALL, shortest first.
    let now = 1_800_000_000; // in a real app: the current unix time in seconds
    let ledger_for_this_job = saved.agree(d, ConsentTerm::ThisTime, now);
    assert!(client.start(&plan, &ledger_for_this_job).is_ok());
    // "This time" is not remembered: `saved` is unchanged.
    assert!(plan.needs_consent(&saved));
}
```

The rules for your prompt:

- Show `plan.disclosure()`: the destination, and what is sent (the kind of each input and its
  size). Nothing else about the request needs to be shown.
- Offer the four terms in `ConsentTerm::ALL`: this time, one hour, one day, until turned off. The
  words are yours, so you can translate them. `ConsentTerm::id()` is the stable name.
- Start the job with the ledger that `agree` returns. Save your ledger only when
  `term.remembers()` is true. "This time" must leave nothing on disk.
- Keep the ledger in `client.consent_file()` (`load_or_empty`, `save`). It is per app, so a yes in
  one app never covers another.
- Do not build your own "are you sure" for online services. A second prompt with different
  wording is a bug in the person's eyes.

## 8. Start, watch, cancel

`start` checks consent again, reads the key (only now), builds the adapter and submits the
job. It returns a `Job` you poll.

<!-- snippet: job -->
```rust
use std::time::Duration;

use craft_ai_core::{ConsentLedger, Event, JobState};

let plan = client.plan(&role, request)?;
let job = client.start(&plan, &ConsentLedger::default())?;

let final_state = loop {
    let snapshot = job.poll()?;
    for event in &snapshot.events {
        match event {
            Event::TextDelta { text } => print!("{text}"),
            Event::Output(output) => println!("result: {:?}", output.payload),
            Event::Notice { message } => println!("note: {message}"),
            Event::ToolCall { .. } => {}
        }
    }
    if snapshot.state.is_terminal() {
        break snapshot.state;
    }
    std::thread::sleep(Duration::from_millis(5)); // in an app: poll from your own timer
};
assert_eq!(final_state, JobState::Succeeded);
```

- **Keep it off the interface thread.** Adapters are synchronous. `start` can do real work
  before it returns, such as reading a key, building the adapter or calling a service, so call it and `poll` from a
  worker or your own job system. `Job::wait(interval, timeout)` exists for scripts and tests.
- **States:** `Queued`, `Running { percent }` (percent is optional), then one of `Succeeded`,
  `Failed { error }` or `Cancelled`. A finished state never changes.
- **Events** arrive once. Each poll returns only what is new since the last poll, so collect them
  as you go. Results are `Event::Output`.
- **Cancel** with `job.cancel()`. It can be called twice and on a finished job. A connection that
  cannot stop an unfinished job returns `Unsupported`: ComfyUI does so unless the app turned on
  `with_targeted_interrupt(true)`, because an older server may treat the interrupt as global. When
  cancel succeeds, wait for the `Cancelled` state before you let go of any picture the job was
  reading. When it does not, wait for any end state and discard the result.
- **Apply the result yourself.** The layer returns bytes or text. Writing it into a document,
  with undo, is your app's job.

## 9. When the tool cannot run

There are two honest answers, and the layer gives you the data for both. You draw them in your
own theme.

**No model chosen, or the choice stopped working.** Ask for the missing-model popup data. It
holds the reason and a picker with the real ways to fix it:

<!-- snippet: missing_model -->
```rust
use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::AppId;

let desk = ToolDesk::new(
    AiClient::new(AppId::new("notes")?, shared.clone(), keys.clone()),
    shared.clone(),
    None,
);
let popup = desk.missing_model(&role, &craft_ai_features::builtin())?;
// `Some` means the tool cannot run in this app; `None` means it can.
assert!(popup.is_some()); // the "notes" app ships no adapter for "tidy"
```

Choosing a model in your popup goes through `ToolDesk::choose`, which sets the default for the
whole suite. Refusal is atomic: if the model cannot do the job, nothing is written.

**The person wants a different model only in your app.** `client.bind(&role, &connection, &model)`
sets an override for your app and `client.clear_binding(&role.id)` removes it. An override
beats the suite default. A broken override is an error. It never falls back to something else,
because that could send data somewhere the person did not choose.

Setting up a model (downloads, keys, a local server) belongs to the launcher's Set up AI window or
to your own Preferences, built from the same shared pieces. Do not build a second setup flow. If you
have no setup screen, your popup links to the launcher.

If you do draw the set-up screen for a built-in model, the shared state keeps the steps in order,
so you draw them and run the work on a thread:

1. `FolderDesk::prepare_native_setup` builds the plan, and `SetupSession::new` holds it. The plan
   says the size, the destination and whether any file needs a licence agreement
   (`NativeSetupPlan::needs_agreement`). Show the tick box only then. With no model folder
   chosen the plan has no destination and `blocker` says `NeedsFolder`. Show the suggested folder
   and let the person take it.
2. Only when the person takes the suggested folder, call `FolderDesk::ensure_default`, then give
   `SetupSession::review` a fresh plan. It takes the suggested folder only when the person has
   none, and never replaces one they chose. Do not call it while drawing the first screen: it
   makes the folder. `FolderDesk::add` links an existing folder for discovery; it does not select
   that folder for downloads. Call `set_default` only when the person explicitly chooses it as
   the destination. Removing a default never promotes another linked folder automatically.
3. One button calls `SetupSession::start`. It checks the files already on disk when there are
   complete ones, and downloads when there are not. When it does nothing, `SetupSession::blocker`
   says why (busy, no folder, agreement missing, this app cannot run it). If a check of the
   files on disk fails, `blocker` asks for a new empty model folder. Use the host's folder picker
   and `FolderDesk::add_new` and `set_default`, then review the new plan. The old files stay put.
   Only the newly approved destination may be downloaded to. Run the returned `SetupRequest` with
   `SetupRequest::run` on a worker, and report progress and the answer with
   `report_progress` and `complete`.
4. When `session.checked()` has files, `ToolDesk::adopt` makes them a model the tool can use. It
   accepts only checked files. It binds the tool only if no choice exists. If one does,
   `Adopted::kept` names it, so you can ask before calling `ToolDesk::choose`.
5. `ToolDesk::start_sample` and `ToolDesk::finish` try it. A built-in selection model gets a click
   on the test picture, because every built-in route reads clicks and some read no words. A
   selection that chose nothing fails the test, and so does a selection that returned a mask with
   no result beside it, because the contract gives every route both.

Nothing in these steps binds a model, downloads anything or starts a job without the person
pressing the button.

## 10. Errors, in one table

A failed job carries an `AiError { kind, message }`. The message is safe to show.

| `ErrorKind` | Show | Then |
|---|---|---|
| `ConsentRequired` | Your consent prompt | Retry with the agreed ledger. |
| `Auth` | The message (it names the connection) | Send the person to Set up AI to fix the key. |
| `RateLimited` | "Try again shortly" | Wait `retry_after_secs` if present, then retry. |
| `Quota` | The message | Do not retry. It needs money or time. |
| `Network` | The message | Retry is reasonable. `error.is_retryable()` says so. |
| `Provider` | The message | One retry at most. Keep the log line. |
| `Unsupported` | The message | Offer step 9. |
| `InvalidRequest` | The message | Your bug, or bad input. |
| `Cancelled` | Nothing | The person did this. Not an error. |
| `Internal` | A short apology and the message | File a bug. The layer reports these and never panics. |

## 11. History and privacy

`AiClient::standard` keeps a shared log of jobs: which app, which connection, what kind of data,
how long, what it cost. It does **not** keep the text of prompts unless you call
`keep_prompts_in_history(true)`. Leave that off. `client.history()` reads it for every app and
`clear_history()` empties it. `without_history()` stops logging.

## 12. Widgets, if you want them

If your app is egui and has no chrome of its own, `craft-ai-egui` has theme-neutral status,
picker, consent prompt, progress, connection panel and history widgets. Hosts with their own
look should draw their own views over the same data. The data and the rules are shared. The
look is yours.

An app adopting the widgets repeats the complete `[patch.crates-io]` recipe in its own root:
`storytold/egui` at `6d518fc799681571963842f95b55a6b68431b15a` and `storytold/winit` at
`a517b3846ee8457d84ee4975110bfda6797c44dc`. Patches do not propagate from this repository. The
library does not force bundled fonts; enable eframe's `default_fonts` or supply your own provider.
Use only this workspace's `craft-ai-core` package, not the separate `craft-libs` copy. The small
[consumer smoke](../../consumer-smoke/README.md) workspace in this checkout checks that recipe and
type identity; its locked check and one draw test pass headlessly. Native focus, IME, clipboard and
rendering still need your own platform test.

## 13. Test your integration with no network

Build a world with no network: a temporary store, an in-memory key store, the launcher's choice
written with `ToolDesk::choose`, and a scripted adapter registered on a `ProviderFactory`. The
test file `crates/ai-client/tests/support/adopting_scene.rs` does exactly this, and every block on
this page runs on top of it.

<!-- snippet: two_apps -->
```rust
use craft_ai_client::{AiClient, RoleStatus};
use craft_ai_core::AppId;

// `client` ships an adapter for the chosen service, so the suite default is ready with no setup.
assert!(matches!(client.status(&role), RoleStatus::Ready(_)));

// Another app reads the same store but ships no adapter, so it is unavailable, and says so.
// `shared` and `keys` are the temporary store and key store this test built.
let second = AiClient::new(AppId::new("lightcraft")?, shared.clone(), keys.clone());
assert!(matches!(
    second.status(&role),
    RoleStatus::Unavailable { .. }
));
```

The tests an adopting app should have are in the [checklist](checklist.md). A Ready badge
proves a route resolves. It does not prove the tool works. Run the real model through your own
tool and check what lands in the document.

## What not to do

- Do not store, read or print a key. The layer reads it inside `start` and nowhere else.
- Do not hard-code a model id, a provider or an address. Read the choice.
- Do not fall back to a different model when the chosen one fails.
- Do not show engine or hardware words (Candle, Burn, CUDA, VRAM, wgpu) to a person.
- Do not call a tool "ready" because files are downloaded or a key is saved. It is ready when
  `status` says so and a real run produced the right result.
- Do not write a toolbar, a prompt bar or a model list into a shared crate. Those are the app's.

Next: [Declare a new tool](declare-a-tool.md), [Bring a service or model](bring-a-service.md),
or go to the [checklist](checklist.md).
