# Checklist

Use the section that matches what you built. Tick a box only when you ran the thing and saw it.
"It compiles" and "the test passes" are not the same as "the tool works in the app".

## An app that uses a tool

### Design

- [ ] The tool's role comes from the catalogue (`Feature::role()`), or is a declared tool whose id
      was agreed with the other maintainers.
- [ ] The app id is the lower-case name the launcher uses. It never changes.
- [ ] The app starts and every other feature works with no connection at all.

### Behaviour

- [ ] `status` is called each time the tool is about to be used, not once at start-up.
- [ ] `Unavailable` greys the tool out and shows the reason as given. It is not drawn as an error.
- [ ] The missing-model popup (`ToolDesk::missing_model`) opens from a greyed-out tool, and
      choosing a model there goes through `ToolDesk::choose`.
- [ ] `plan` runs before any consent prompt. Nobody is asked to agree to a request the app cannot run.
- [ ] The consent prompt shows `plan.disclosure()` and offers `ConsentTerm::ALL`.
- [ ] "This time" leaves nothing on disk. Longer terms are saved only when `term.remembers()`.
- [ ] `start` and `poll` run off the interface thread.
- [ ] Progress is shown. Cancel works while the job is queued and while it runs, and the app waits
      for `Cancelled` before releasing the input.
- [ ] Every `ErrorKind` has a path on screen (see the table in the app guide). `Cancelled` shows nothing.
- [ ] The result is applied through the app's own undo, and survives save and reopen.

### Safety

- [ ] The app never reads, stores, prints or logs a key.
- [ ] No model id, provider name or address is hard-coded. A broken binding is an error, never
      a reason to use a default.
- [ ] No text a person can read contains engine names or hardware words.
- [ ] `keep_prompts_in_history` is off, unless the person turned it on.
- [ ] Tests set `CRAFT_AI_DIR` (or use a temporary store). None of them touches real settings.

### Tests to write

Each of these catches a real class of bug. Use a scripted adapter and a temporary store.

- [ ] **Unbound:** no model chosen, so `status` is `Unavailable`, and nothing is sent.
- [ ] **Bound:** the suite default is used, and the adapter receives the right request.
- [ ] **A second app id** reads the same default without any setup.
- [ ] **An app override** beats the default, and `clear_binding` restores the default.
- [ ] **No adapter:** the app that does not ship the adapter is `Unavailable`, `plan` is
      `Unsupported`, no key is read and the other app's adapter saw nothing.
- [ ] **A remote connection** gives `ConsentRequired` and builds no adapter until agreed.
- [ ] **A removed connection** is an error. It is not "unbound".
- [ ] **Cancel** ends in `Cancelled` and does not disturb another job.
- [ ] **Old store file:** a file from before custom tools existed still loads.

Watch each behavioural test fail against a stub first, so you know it can.

### Proof

- [ ] A real model was run through **your own tool** and the stored result checked: its size,
      its content, and that it is still there after save and reopen.
- [ ] You say which of these you did: scripted test, local server, real service, inside the app.
      Never describe one as another.

## A new tool

- [ ] The capability is the narrowest honest one (`image.describe`, not `image.analyse`).
- [ ] `inputs` lists exactly what the tool sends, and no more.
- [ ] The id is registered (`register_tool`) if more than one app will use it.
- [ ] Two apps that declare the same id agree on its capability and inputs. A mismatch is refused.
- [ ] At least one model was chosen for it with `ToolDesk::choose`, and `choose` refused a model that
      could not do the job.
- [ ] The result shape is written down (text, media type, or JSON) for the next person.
- [ ] If it should be easy to set up, a catalogue entry has size, SHA-256, source and licence for
      every file, and a statement that licences are disclosed and not checked.

## A service or a model

- [ ] `models()` lists only what the service really does. Capabilities and inputs are not guessed
      from a model's name.
- [ ] A request the model cannot take is refused in `submit`, before anything is sent.
- [ ] The conformance checks pass: `run_lifecycle`, `run_cancel`, `run_unknown_job`, `run_rejects`.
- [ ] Remote means `locality: remote`, and the endpoint is HTTPS (or loopback).
- [ ] The key arrives through the connection's secret name. It is never in an error, a log or an event.
- [ ] HTTP agents come from `craft-ai-http`.
- [ ] Errors use the right `ErrorKind`, and `RateLimited` carries `retry_after_secs` when known.
- [ ] You tested a failure: a wrong key, a full quota, a dropped connection, a cancelled job.
- [ ] The docs for your adapter say what ran: conformance, scripted server, the real service, or
      inside an app.

## Before you open a pull request here

- [ ] Targeted cargo only, one process at a time: `cargo test -p <crate> -j 8`. No workspace builds.
- [ ] `cargo fmt --all --check` and `cargo clippy -p <crate> --all-targets --locked -j 8 -- -D warnings`.
- [ ] No file over 500 lines. No `unwrap`, `expect` or `panic` in shipped code.
- [ ] No personal folder paths in anything you commit.
- [ ] The pull request says plainly what was run and what was not.
