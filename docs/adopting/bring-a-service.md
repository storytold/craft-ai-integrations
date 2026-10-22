# Bring a service or a model

Use this page when you want something to **run** a tool: a hosted API, a server on the person's
computer, or a model file. Pick the shortest route that works.

| You have | Write | Rust? |
|---|---|---|
| A hosted API with plain request and reply (or submit and poll) | One settings file for the declarative adapter | No |
| A hosted or local API with another shape | A `Provider`, registered by name | Yes |
| A model file that should run inside the app, on this computer | A `ModelRunner` for a built-in engine | Yes |
| A ComfyUI workflow | Import it in Set up AI (see the [status page](../STATUS.md) for which tools this covers) | No |

Every Rust block here is a real test (`cargo test -p craft-ai-client --test adopting_tool_guides`), and
`--test adopting_drift` fails if a block differs from the tested code.

You never edit an app. An app that does not ship your adapter reports your service as
unavailable, in plain words, and asks for no consent and reads no key.

## Route 1: a settings file

If the API is "send JSON, get JSON" or "submit, then poll", describe it as data. No code runs
from the file. Read [the design](../design/declarative-adapter.md) for the format and its limits.
It does not cover signed requests, token refresh, file uploads or streaming.

## Route 2: write a `Provider`

A `Provider` has five methods:

| Method | Must |
|---|---|
| `models()` | List the models, each with the capabilities and inputs it really supports. This is also the "test connection" call. |
| `estimate(model, request)` | Optional. A price estimate when you can give one. |
| `submit(model, request)` | Check the request against the model's spec and refuse **before sending anything** with `Unsupported` or `InvalidRequest`. Then start the job and return its id. |
| `poll(job)` | Return the state and only the events that are new since the last poll. A finished state must stay the same. |
| `cancel(job)` | Stop the job. Calling it twice, or on a finished job, is not an error. An unknown job is. |

The trait is synchronous on purpose. Do the slow work on your own thread and let `poll` read the
state. Here is a complete one, small enough to read in a minute. It "shouts" a prompt back:

<!-- snippet: shouter_provider -->
```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use craft_ai_core::payload::Limits;
use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, Event, InputRole, JobId, JobSnapshot,
    JobState, ModelId, ModelInfo, Output, Payload, Provider, Request,
};

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Queued,
    Running,
    Done,
    Cancelled,
}

struct Job {
    phase: Phase,
    answer: String,
}

struct Shouter {
    model: ModelInfo,
    jobs: Mutex<HashMap<String, Job>>,
    next: AtomicU32,
}

impl Shouter {
    fn new() -> Result<Self, AiError> {
        let capability = CapabilityId::parse("text.complete").map_err(bad)?;
        let spec = CapabilitySpec::new(capability)
            .accepting(InputRole::Prompt, 1)
            .requiring(InputRole::Prompt);
        let id = ModelId::new("shout-1").map_err(bad)?;
        Ok(Self {
            model: ModelInfo::new(id, "Shout 1").with(spec),
            jobs: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Job>>, AiError> {
        self.jobs
            .lock()
            .map_err(|_| AiError::new(ErrorKind::Internal, "the job list is unavailable"))
    }
}

fn bad(e: impl std::fmt::Display) -> AiError {
    AiError::new(ErrorKind::Internal, e.to_string())
}

fn unknown() -> AiError {
    AiError::invalid("that job does not exist")
}

impl Provider for Shouter {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(vec![self.model.clone()])
    }

    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        // Refuse first. Nothing has been sent anywhere yet.
        if *model != self.model.id {
            return Err(AiError::unsupported("this service has no such model"));
        }
        let spec = self.model.spec_for(&request.capability).ok_or_else(|| {
            AiError::unsupported(format!("this model cannot do {}", request.capability))
        })?;
        request.validate(spec, &Limits::default())?;
        let answer = match request.inputs.first().map(|i| &i.payload) {
            Some(Payload::Text { text }) => text.to_uppercase(),
            _ => return Err(AiError::invalid("a text prompt is needed")),
        };
        let name = format!("job-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.lock()?.insert(
            name.clone(),
            Job {
                phase: Phase::Queued,
                answer,
            },
        );
        JobId::new(&name).map_err(bad)
    }

    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        let mut jobs = self.lock()?;
        let entry = jobs.get_mut(job.as_str()).ok_or_else(unknown)?;
        Ok(match entry.phase {
            Phase::Queued => {
                entry.phase = Phase::Running;
                JobSnapshot::new(JobState::Running { percent: Some(50) })
            }
            Phase::Running => {
                entry.phase = Phase::Done;
                let mut done = JobSnapshot::new(JobState::Succeeded);
                done.events.push(Event::Output(Output {
                    payload: Payload::text(entry.answer.clone()),
                    label: None,
                }));
                done // the answer is delivered once
            }
            Phase::Done => JobSnapshot::new(JobState::Succeeded),
            Phase::Cancelled => JobSnapshot::new(JobState::Cancelled),
        })
    }

    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        let mut jobs = self.lock()?;
        let entry = jobs.get_mut(job.as_str()).ok_or_else(unknown)?;
        if matches!(entry.phase, Phase::Queued | Phase::Running) {
            entry.phase = Phase::Cancelled;
        }
        Ok(())
    }
}

// Prove it with the checks every provider must pass.
use craft_ai_core::conformance::{run_cancel, run_lifecycle, run_rejects, run_unknown_job};

let provider = Shouter::new()?;
let model = provider.model.id.clone();
let capability = CapabilityId::parse("text.complete")?;
let good = Request::new(capability.clone()).with_input(InputRole::Prompt, Payload::text("hi"));
let no_prompt = Request::new(capability);

run_lifecycle(&provider, &model, &good)?;
run_cancel(&provider, &model, &good)?;
run_unknown_job(&provider)?;
run_rejects(&provider, &model, &no_prompt, ErrorKind::InvalidRequest)?;
```

Rules your provider must keep (the checks above test most of them):

- **Refuse before sending.** A request the model cannot take is rejected in `submit`, with nothing
  on the wire.
- **Keep time moving forward.** A job goes `Queued`, `Running`, then ends. Progress never goes
  down. A finished job stays finished.
- **Deliver events once.** Each result appears in exactly one poll.
- **Never put a key in an error, a log line or an event.** Use `AiError::redacted(kind, message,
  &[&secret])` when a reply might echo the key back.
- **Use the platform's HTTP setting.** Build agents through `craft-ai-http`. A test in this
  repository fails if a crate builds one without it.
- **Say which kind of failure it is.** `Auth`, `RateLimited` (with `with_retry_after`), `Quota`,
  `Network` and `Provider` each lead to a different thing on screen.

### Register it

The name you register is the `provider` field of a connection. Registering is the whole of
adding an adapter to an app:

<!-- snippet: register_adapter -->
```rust
use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::mock::{MockProvider, Outcome, Script};
use craft_ai_core::{AppId, Provider};

let mut factory = ProviderFactory::standard();
factory.register("acme", |connection, key| {
    // `key` is already read from the key store. The adapter never touches a keychain.
    // A real adapter sends `key` with each request. This stand-in has nowhere to send it.
    let _ = key;
    let script = Script {
        running_polls: 1,
        text_chunks: vec![],
        outcome: Outcome::Succeed(vec![]),
    };
    Ok(Arc::new(MockProvider::new(connection.models.clone(), script)) as Arc<dyn Provider>)
});
assert!(factory.knows("acme"));
assert!(!ProviderFactory::standard().knows("acme"));
let app =
    AiClient::new(AppId::new("wordcraft")?, shared.clone(), keys.clone()).with_factory(factory);
```

### Describe the connection

A person adds the connection in Set up AI. By hand, or from code, it is data:

```json
{
  "id": "acme",
  "label": "Acme",
  "provider": "acme",
  "endpoint": "https://api.acme.example/v1",
  "locality": "remote",
  "auth": { "kind": "api_key", "secret": "acme-key" },
  "billing": "metered",
  "models": [ { "id": "acme-vision-1", "label": "Acme Vision" } ]
}
```

- `locality: "remote"` means anything sent needs the person's agreement first. Use `"local"`
  only for something that stays on this computer.
- `auth.secret` names a key in the key store. It is a name, not the key. Keys never go in this
  file, in an environment variable you set for the person, or on a command line.
- Remote endpoints must use HTTPS. Plain HTTP is accepted only for a loopback address.

## Route 3: a model that runs inside the app

For a model file that should run on this computer with nothing to install, write a
`ModelRunner` for a built-in engine. The job queue, threads, Cancel, progress, loading the
weights once and turning a crash into a failed job are already done. See
[Add a model to a built-in engine](../ADD-AN-ENGINE.md). Its code is compiled in that crate's tests,
like this page.

If the person's model should come from somewhere else (their own ComfyUI, a different engine),
that is another connection for the same tool. No app changes.

## Be honest about what you ran

A scripted test shows your adapter follows the contract. It does not show the real service
works. When you publish an adapter, say which of these you did:

- ran the conformance checks against your adapter;
- ran it against a scripted server on this computer;
- ran it against the real service, once, with a stated model and input;
- ran it from inside an app and checked what landed in the document.

Do not describe one as another. The repository's [status page](../STATUS.md) uses these levels.

Next: the [checklist](checklist.md).
