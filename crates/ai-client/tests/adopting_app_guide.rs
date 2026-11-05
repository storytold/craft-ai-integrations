//! Runs every Rust block in `docs/adopting/use-in-your-app.md`.
//!
//! Each test is one block. `adopting_drift` fails if the guide and the block differ.

// A block shows only the variables it needs, so the rest of the scene stays unused.
#![allow(unused_variables)]

#[path = "support/adopting_scene.rs"]
mod adopting_scene;

use adopting_scene::{Scene, TestResult, scripted_client, tighten_request};

// Compiles only: it would touch the person's real settings if it ran.
#[allow(dead_code)]
fn standard_client() -> TestResult {
    // snippet: standard_client
    use craft_ai_client::AiClient;
    use craft_ai_core::AppId;

    // Reads the shared connections file the launcher writes, and keys from the environment
    // and then the OS keychain.
    let client = AiClient::standard(AppId::new("wordcraft")?)?;
    // end snippet
    Ok(())
}

#[test]
fn catalogue_role() -> TestResult {
    // snippet: catalogue_role
    use craft_ai_features::builtin;

    let select_subject = builtin()
        .into_iter()
        .find(|feature| feature.id == "select-subject")
        .and_then(|feature| feature.role())
        .ok_or("the catalogue has no such tool")?;
    assert_eq!(select_subject.capability.as_str(), "image.segment");
    // end snippet
    Ok(())
}

#[test]
fn status() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: status
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
    // end snippet
    Ok(())
}

#[test]
fn request() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: request
    use craft_ai_core::{InputRole, Payload, Request};

    let request = Request::new(role.capability.clone())
        .with_input(InputRole::Prompt, Payload::text("Tighten this paragraph."))
        .with_param("tone", serde_json::json!("plain"));
    // end snippet
    Ok(())
}

#[test]
fn plan() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: plan
    let plan = client.plan(&role, request)?;
    println!("{} will use {}", role.label, plan.connection_label());
    // end snippet
    Ok(())
}

#[test]
fn consent() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(true)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: consent
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
    // end snippet
    Ok(())
}

#[test]
fn job() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: job
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
    // end snippet
    Ok(())
}

#[test]
fn missing_model() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: missing_model
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
    // end snippet
    Ok(())
}

#[test]
fn two_apps() -> TestResult {
    let Scene {
        _dir,
        shared,
        keys,
        role,
        model,
        launcher,
    } = Scene::new(false)?;
    let client = scripted_client(&shared, &keys)?;
    let request = tighten_request(&role);
    // snippet: two_apps
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
    // end snippet
    Ok(())
}
