//! Choosing a model for a tool and proving it does the job, against fake providers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory, RoleStatus, SampleReport, TestStart, ToolDesk};
use craft_ai_core::job::{Event, Output};
use craft_ai_core::{
    AiError, Connection, ConnectionId, Disclosure, ErrorKind, JobId, JobSnapshot, JobState,
    ModelId, ModelInfo, Payload, Provider, Request, Role, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::ConsentFile;
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// Answers every job at once with whatever its connection's id says.
struct Fake {
    how: String,
    jobs: Mutex<usize>,
}

impl Provider for Fake {
    /// No model list; the connection carries it.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Counts the job and gives it a number.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        let mut n = self.jobs.lock().unwrap();
        *n += 1;
        JobId::new(format!("job-{n}")).map_err(|e| AiError::invalid(e.to_string()))
    }
    /// A finished job whose result depends on the connection's id: an image, text, nothing, or a failure.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        let output = |payload| {
            Event::Output(Output {
                payload,
                label: None,
            })
        };
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        match self.how.as_str() {
            "good" => snap
                .events
                .push(output(Payload::inline("image/png", vec![1, 2, 3])?)),
            "texty" => snap.events.push(output(Payload::text("I cannot draw"))),
            "empty" => {}
            _ => {
                snap = JobSnapshot::new(JobState::Failed {
                    error: AiError::new(ErrorKind::Provider, "the service said no"),
                });
            }
        }
        Ok(snap)
    }
    /// Nothing to stop: the job is already done.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

/// A connection named `id`, online or on this computer, with the classified `models`.
fn connection(id: &str, remote: bool, enabled: bool, models: &[&str]) -> Connection {
    let models: Vec<_> = models
        .iter()
        .filter_map(|m| craft_ai_openai::catalog::classify(m))
        .collect();
    serde_json::from_value(json!({
        "id": id, "label": format!("Connection {id}"), "provider": "fake-image",
        "endpoint": if remote { "https://api.example.com/v1" } else { "http://127.0.0.1:9/v1" },
        "locality": if remote { "remote" } else { "local" },
        "auth": if remote { json!({"kind":"api_key","secret": format!("{id}-key")}) } else { json!({"kind":"none"}) },
        "billing": if remote { "metered" } else { "free" },
        "enabled": enabled,
        "models": models,
    }))
    .unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    desk: ToolDesk,
    consent: ConsentFile,
    /// Providers built so far, per world, so parallel tests cannot disturb each other's count.
    builds: Arc<AtomicUsize>,
}

/// A temporary store holding `connections`, secrets for them, and a desk whose fake provider counts how often one is built.
fn world(connections: Vec<Connection>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: connections.clone(),
            ..StoreFile::default()
        })
        .unwrap();
    let secrets = Arc::new(MemoryStore::new());
    for c in &connections {
        if let Some(r) = c.auth.secret_ref() {
            secrets
                .set(
                    &SecretRef::new(r.as_str()).unwrap(),
                    &SecretString::new("k"),
                )
                .unwrap();
        }
    }
    let builds = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&builds);
    let mut factory = ProviderFactory::standard();
    factory.register("fake-image", move |c, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        // `good-remote` and `good-local` both behave as `good`.
        let how = c.id.as_str().split('-').next().unwrap_or("").to_owned();
        Ok(Arc::new(Fake {
            how,
            jobs: Mutex::new(0),
        }))
    });
    let consent = ConsentFile::new(dir.path().join("consent.json"));
    let client =
        AiClient::new(ToolDesk::app().unwrap(), shared.clone(), secrets).with_factory(factory);
    let desk = ToolDesk::new(
        client,
        shared.clone(),
        Some(ConsentFile::new(dir.path().join("consent.json"))),
    );
    World {
        _dir: dir,
        shared,
        desk,
        consent,
        builds,
    }
}

/// The built-in tool `id` as a role.
fn tool(id: &str) -> Role {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
        .role()
        .unwrap()
}

/// The built-in sample request for tool `id`.
fn sample(id: &str) -> Request {
    let f = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap();
    craft_ai_features::sample_request(&f).unwrap()
}

/// A connection id from text.
fn cid(s: &str) -> ConnectionId {
    ConnectionId::new(s).unwrap()
}

/// The first model offered for the image tool.
fn image_model(w: &World) -> ModelId {
    w.desk.view(&tool("generate-image")).unwrap().choices[0]
        .model
        .clone()
}

/// Only models that can do the job, on connections that are switched on, are offered.
#[test]
fn only_models_that_can_do_the_job_on_connections_that_are_on_are_offered() {
    let w = world(vec![
        connection("good-local", false, true, &["gpt-image-1", "gpt-4o"]),
        connection("good-off", false, false, &["gpt-image-1"]),
        connection("textonly-local", false, true, &["gpt-4o"]),
    ]);
    let view = w.desk.view(&tool("generate-image")).unwrap();
    let offered: Vec<_> = view
        .choices
        .iter()
        .map(|c| (c.connection.as_str(), c.model.as_str()))
        .collect();
    assert_eq!(offered, [("good-local", "gpt-image-1")]);
    assert!(view.bound.is_none());
    assert!(matches!(view.status, RoleStatus::Unavailable { .. }));
    // A tool whose inputs no model takes has no choices at all.
    assert!(
        w.desk
            .view(&tool("generative-expand"))
            .unwrap()
            .choices
            .is_empty()
    );
}

/// Choosing binds the tool, and the status follows.
#[test]
fn choosing_binds_the_tool_and_the_status_follows() {
    let w = world(vec![connection(
        "good-local",
        false,
        true,
        &["gpt-image-1"],
    )]);
    let role = tool("generate-image");
    let model = image_model(&w);
    w.desk.choose(&role, &cid("good-local"), &model).unwrap();
    let view = w.desk.view(&role).unwrap();
    assert_eq!(view.bound, Some((cid("good-local"), model)));
    assert!(matches!(view.status, RoleStatus::Ready(_)));
    // The choice is the launcher's default, so it is in the shared store for apps to inherit.
    assert_eq!(w.shared.load().unwrap().defaults.len(), 1);
    w.desk.clear(&role.id).unwrap();
    assert!(w.desk.view(&role).unwrap().bound.is_none());
    assert!(w.shared.load().unwrap().defaults.is_empty());
}

/// A model that cannot do the job is refused and nothing is written.
#[test]
fn a_model_that_cannot_do_the_job_is_refused_and_nothing_is_written() {
    let w = world(vec![connection(
        "good-local",
        false,
        true,
        &["gpt-image-1", "gpt-4o"],
    )]);
    let err = w
        .desk
        .choose(
            &tool("generate-image"),
            &cid("good-local"),
            &ModelId::new("gpt-4o").unwrap(),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    assert!(w.shared.load().unwrap().defaults.is_empty());
    assert!(
        w.desk
            .choose(
                &tool("generate-image"),
                &cid("nope"),
                &ModelId::new("gpt-image-1").unwrap()
            )
            .is_err()
    );
}

/// A choice that stops holding says why and stays visible.
#[test]
fn a_choice_that_stops_holding_says_why_and_stays_visible() {
    let w = world(vec![connection(
        "good-remote",
        true,
        true,
        &["gpt-image-1"],
    )]);
    let role = tool("generate-image");
    let model = image_model(&w);
    w.desk.choose(&role, &cid("good-remote"), &model).unwrap();
    // The connection is switched off afterwards.
    w.shared
        .update(|s| s.connections[0].enabled = false)
        .unwrap();
    let view = w.desk.view(&role).unwrap();
    assert!(view.bound.is_some());
    match view.status {
        RoleStatus::Unavailable { reason } => assert!(reason.contains("switched off"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

/// Runs the sample for tool `id` to its end; `None` when it stopped to ask for agreement.
fn run(
    w: &World,
    id: &str,
    reviewed: Option<&Disclosure>,
) -> Result<Option<SampleReport>, AiError> {
    Ok(match w.desk.start_test(&tool(id), sample(id), reviewed)? {
        TestStart::Started(job) => Some(ToolDesk::finish(&tool(id), &job, Duration::from_secs(5))),
        TestStart::NeedsConsent { .. } => None,
    })
}

/// Chooses the image model on connection `conn`.
fn bound(w: &World, conn: &str) {
    let role = tool("generate-image");
    w.desk.choose(&role, &cid(conn), &image_model(w)).unwrap();
}

/// A sample on this computer runs at once and passes when the right kind of thing comes back.
#[test]
fn a_local_sample_runs_at_once_and_passes_when_the_right_kind_of_thing_comes_back() {
    let w = world(vec![connection(
        "good-local",
        false,
        true,
        &["gpt-image-1"],
    )]);
    bound(&w, "good-local");
    match run(&w, "generate-image", None).unwrap().unwrap() {
        SampleReport::Passed { summary, .. } => assert!(summary.contains("image"), "{summary}"),
        other => panic!("{other:?}"),
    }
}

/// A sample on an online service asks first and builds no provider; agreeing runs it and records the agreement.
#[test]
fn a_remote_sample_waits_for_agreement_and_builds_nothing_until_then() {
    let w = world(vec![connection(
        "good-remote",
        true,
        true,
        &["gpt-image-1"],
    )]);
    bound(&w, "good-remote");
    let before = w.builds.load(Ordering::SeqCst);
    let shown = match w
        .desk
        .start_test(&tool("generate-image"), sample("generate-image"), None)
        .unwrap()
    {
        TestStart::NeedsConsent {
            disclosure,
            connection_label,
            billing,
        } => {
            assert_eq!(disclosure.destination, "https://api.example.com");
            assert_eq!(connection_label, "Connection good-remote");
            assert_eq!(billing, craft_ai_core::Billing::Metered);
            disclosure
        }
        TestStart::Started(_) => panic!("sent before the person agreed"),
    };
    assert_eq!(
        w.builds.load(Ordering::SeqCst),
        before,
        "no provider was even built"
    );
    assert!(!w.consent.path().exists(), "nothing recorded");
    // Agreeing runs it and records the agreement, so the next sample needs no question.
    assert!(matches!(
        run(&w, "generate-image", Some(&shown)).unwrap().unwrap(),
        SampleReport::Passed { .. }
    ));
    assert!(w.consent.path().exists());
    assert!(matches!(
        run(&w, "generate-image", None).unwrap().unwrap(),
        SampleReport::Passed { .. }
    ));
}

/// A model that answers with the wrong kind, nothing, or an error fails the check and says which.
#[test]
fn a_model_that_answers_wrongly_or_not_at_all_fails_the_check() {
    for (conn, expect) in [
        ("texty-local", "other than image"),
        ("empty-local", "returned nothing"),
        ("broken-local", "the service said no"),
    ] {
        let w = world(vec![connection(conn, false, true, &["gpt-image-1"])]);
        bound(&w, conn);
        match run(&w, "generate-image", None).unwrap().unwrap() {
            SampleReport::Failed { message } => {
                assert!(message.contains(expect), "{conn}: {message}")
            }
            other => panic!("{conn}: {other:?}"),
        }
    }
}

/// A sample for a tool with no choice is refused before any provider is built.
#[test]
fn a_sample_with_no_choice_made_is_refused_before_anything_is_sent() {
    let w = world(vec![connection(
        "good-remote",
        true,
        true,
        &["gpt-image-1"],
    )]);
    let before = w.builds.load(Ordering::SeqCst);
    let err = run(&w, "generate-image", None).err().unwrap();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert_eq!(w.builds.load(Ordering::SeqCst), before);
}

/// Choosing again replaces the old choice, so a tool never holds two.
#[test]
fn choosing_again_replaces_the_old_choice_rather_than_adding_to_it() {
    let w = world(vec![
        connection("good-local", false, true, &["gpt-image-1"]),
        connection("good-other", false, true, &["gpt-image-1"]),
    ]);
    let role = tool("generate-image");
    let model = image_model(&w);
    w.desk.choose(&role, &cid("good-local"), &model).unwrap();
    w.desk.choose(&role, &cid("good-other"), &model).unwrap();
    assert_eq!(
        w.shared.load().unwrap().defaults.len(),
        1,
        "one default per tool"
    );
    assert_eq!(
        w.desk.view(&role).unwrap().bound,
        Some((cid("good-other"), model))
    );
}

/// The built-in tools named by `ids`, as roles.
fn roles(ids: &[&str]) -> Vec<Role> {
    ids.iter().map(|i| tool(i)).collect()
}

/// Applies what the person agreed to: the offer for `ids`, through `accept_recommended`.
fn say_yes(w: &World, ids: &[&str]) -> craft_ai_client::Accepted {
    let all = roles(ids);
    let offer = w.desk.auto_bind_offer(&all).unwrap();
    w.desk.accept_recommended(&all, &offer).unwrap()
}

/// Offering a model for an unchosen tool writes nothing; the choice is made only on a yes.
#[test]
fn an_offer_for_an_unchosen_tool_writes_nothing_until_the_person_says_yes() {
    let w = world(vec![connection(
        "good-local",
        false,
        true,
        &["gpt-image-1"],
    )]);
    let all = roles(&["generate-image", "generate-video"]);
    let offer = w.desk.auto_bind_offer(&all).unwrap();
    let ids: Vec<_> = offer.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        ids,
        ["generate-image"],
        "the video tool has nothing that can do it"
    );
    assert!(
        w.shared.load().unwrap().defaults.is_empty(),
        "an offer binds nothing"
    );
    assert!(
        w.desk
            .view(&tool("generate-image"))
            .unwrap()
            .bound
            .is_none()
    );
    let done = w.desk.accept_recommended(&all, &offer).unwrap();
    assert_eq!(done.bound.len(), 1);
    let view = w.desk.view(&tool("generate-image")).unwrap();
    assert_eq!(view.bound.map(|(c, _)| c), Some(cid("good-local")));
    assert!(matches!(view.status, RoleStatus::Ready(_)));
    assert!(
        w.desk
            .view(&tool("generate-video"))
            .unwrap()
            .bound
            .is_none()
    );
}

/// The offer prefers a model on this computer to a hosted service.
#[test]
fn the_offer_prefers_this_computer_to_a_hosted_service() {
    let w = world(vec![
        connection("good-remote", true, true, &["gpt-image-1"]),
        connection("good-local", false, true, &["gpt-image-1"]),
    ]);
    say_yes(&w, &["generate-image"]);
    let bound = w.desk.view(&tool("generate-image")).unwrap().bound.unwrap();
    assert_eq!(
        bound.0,
        cid("good-local"),
        "nothing leaves the computer when it need not"
    );
}

/// A choice the person made is never offered over, even one that stopped working.
#[test]
fn the_offer_never_covers_a_choice_the_person_made() {
    let w = world(vec![
        connection("good-remote", true, true, &["gpt-image-1"]),
        connection("good-local", false, true, &["gpt-image-1"]),
    ]);
    let model = image_model(&w);
    w.desk
        .choose(&tool("generate-image"), &cid("good-remote"), &model)
        .unwrap();
    assert!(
        w.desk
            .auto_bind_offer(&roles(&["generate-image"]))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        w.desk
            .view(&tool("generate-image"))
            .unwrap()
            .bound
            .unwrap()
            .0,
        cid("good-remote")
    );
    // Even a choice that stopped working stays, so it is there when the connection comes back.
    w.shared
        .update(|s| s.connections[0].enabled = false)
        .unwrap();
    assert!(
        w.desk
            .auto_bind_offer(&roles(&["generate-image"]))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        w.desk
            .view(&tool("generate-image"))
            .unwrap()
            .bound
            .unwrap()
            .0,
        cid("good-remote")
    );
}

/// A switched-off connection is not offered, and once accepted nothing more is offered.
#[test]
fn the_offer_ignores_a_connection_that_is_switched_off_and_a_second_run_offers_nothing() {
    let w = world(vec![
        connection("good-off", false, false, &["gpt-image-1"]),
        connection("good-remote", true, true, &["gpt-image-1"]),
    ]);
    let first = say_yes(&w, &["generate-image"]);
    assert_eq!(first.bound.len(), 1);
    assert_eq!(
        w.desk
            .view(&tool("generate-image"))
            .unwrap()
            .bound
            .unwrap()
            .0,
        cid("good-remote")
    );
    assert!(
        w.desk
            .auto_bind_offer(&roles(&["generate-image"]))
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.shared.load().unwrap().defaults.len(), 1);
}

/// Choosing "only if unchosen" writes nothing when the tool already has a choice, even if the
/// caller looked before it was made: the check is inside the write.
#[test]
fn choosing_only_if_unchosen_never_overwrites_a_choice_already_made() {
    let w = world(vec![
        connection("good-remote", true, true, &["gpt-image-1"]),
        connection("good-local", false, true, &["gpt-image-1"]),
    ]);
    let model = image_model(&w);
    let role = tool("generate-image");
    assert!(
        w.desk
            .choose_if_unchosen(&role, &cid("good-local"), &model)
            .unwrap(),
        "nothing was chosen, so this one is written"
    );
    assert!(
        !w.desk
            .choose_if_unchosen(&role, &cid("good-remote"), &model)
            .unwrap(),
        "a choice exists now"
    );
    assert_eq!(
        w.desk.view(&role).unwrap().bound.unwrap().0,
        cid("good-local")
    );
}

/// An intervening choice wins even when the model offered earlier has gone away. The rejected
/// offer must not fail validation before noticing that another app already chose the tool.
#[test]
fn an_intervening_choice_wins_over_an_unavailable_offer() {
    let w = world(vec![
        connection("good-local", false, true, &["gpt-image-1"]),
        connection("good-remote", true, true, &["gpt-image-1"]),
    ]);
    let role = tool("generate-image");
    let model = image_model(&w);
    w.desk.choose(&role, &cid("good-local"), &model).unwrap();
    w.shared
        .update(|store| {
            store.connections.retain(|c| c.id != cid("good-remote"));
        })
        .unwrap();
    assert!(
        !w.desk
            .choose_if_unchosen(&role, &cid("good-remote"), &model)
            .unwrap()
    );
    assert_eq!(
        w.desk.view(&role).unwrap().bound.unwrap().0,
        cid("good-local")
    );
}
