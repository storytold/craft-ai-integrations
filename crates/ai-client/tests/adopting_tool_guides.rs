//! Runs every Rust block in `docs/adopting/declare-a-tool.md` and
//! `docs/adopting/bring-a-service.md`.
//!
//! Each test is one block. `adopting_drift` fails if the guide and the block differ.

// A block shows only the variables it needs, so the rest of the scene stays unused.
#![allow(unused_variables)]

#[path = "support/adopting_scene.rs"]
mod adopting_scene;

use adopting_scene::{Scene, TestResult};

#[test]
fn well_known() -> TestResult {
    // snippet: well_known
    use craft_ai_core::capability::well_known;

    let known: Vec<String> = well_known().iter().map(|c| c.to_string()).collect();
    assert!(known.contains(&"image.inpaint".to_string()));
    assert!(known.contains(&"image.segment".to_string()));
    // end snippet
    Ok(())
}

#[test]
fn declare_and_choose() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    // snippet: declare_and_choose
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
    // end snippet
    Ok(())
}

#[test]
fn id_is_a_contract() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    // snippet: id_is_a_contract
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
    // end snippet
    Ok(())
}

#[test]
fn describe_request() -> TestResult {
    // snippet: describe_request
    use craft_ai_core::{CapabilityId, InputRole, Payload, Request};

    let request = Request::new(CapabilityId::parse("image.describe")?).with_input(
        InputRole::Source,
        Payload::inline("image/png", vec![0x89, b'P', b'N', b'G'])?,
    );
    assert_eq!(request.count(&InputRole::Source), 1);
    // end snippet
    Ok(())
}

#[test]
fn shouter_provider() -> TestResult {
    // snippet: shouter_provider
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
    // end snippet
    Ok(())
}

#[test]
fn register_adapter() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    // snippet: register_adapter
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
    // end snippet
    Ok(())
}
