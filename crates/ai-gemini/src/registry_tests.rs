//! Deterministic tracking pressure without a live service or a model.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

fn provider() -> GeminiProvider {
    GeminiProvider::new(GeminiConfig::new(
        "http://127.0.0.1:9/v1beta",
        SecretString::new("scripted-test-key"),
    ))
    .unwrap()
}

fn request() -> Request {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Grayscale);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255]).unwrap();
    }
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", bytes.clone()).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", bytes).unwrap(),
        )
}

fn seed(provider: &GeminiProvider, state: JobState) -> Vec<JobId> {
    let mut jobs = lock(&provider.jobs).unwrap();
    (0..MAX_TRACKED)
        .map(|i| {
            let id = JobId::new(format!("saved-{i}")).unwrap();
            jobs.insert(
                id.clone(),
                Slot {
                    state: state.clone(),
                    events: Vec::new(),
                    terminal_delivered: false,
                },
            );
            id
        })
        .collect()
}

#[test]
fn unpolled_terminal_jobs_survive_tracking_pressure() {
    for state in [
        JobState::Cancelled,
        JobState::Failed {
            error: AiError::invalid("scripted failure"),
        },
        JobState::Succeeded,
    ] {
        let p = provider();
        let ids = seed(&p, state.clone());
        let error = p
            .submit(&ModelId::new(DEFAULT_MODEL).unwrap(), &request())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::RateLimited);
        assert_eq!(lock(&p.jobs).unwrap().len(), MAX_TRACKED);
        for id in ids {
            assert_eq!(p.poll(&id).unwrap().state, state);
            assert_eq!(p.poll(&id).unwrap().state, state);
        }
    }
}

#[test]
fn only_a_delivered_terminal_state_can_be_evicted() {
    let p = provider();
    let ids = seed(&p, JobState::Cancelled);
    // A poll while running does not count as delivery of the later terminal state.
    lock(&p.jobs).unwrap().get_mut(&ids[0]).unwrap().state = JobState::Running { percent: None };
    assert!(!p.poll(&ids[0]).unwrap().state.is_terminal());
    lock(&p.jobs).unwrap().get_mut(&ids[0]).unwrap().state = JobState::Cancelled;
    assert_eq!(p.poll(&ids[1]).unwrap().state, JobState::Cancelled);
    let new = p
        .submit(&ModelId::new(DEFAULT_MODEL).unwrap(), &request())
        .unwrap();
    let jobs = lock(&p.jobs).unwrap();
    assert_eq!(jobs.len(), MAX_TRACKED);
    assert!(!jobs.contains_key(&ids[1]));
    assert!(jobs.contains_key(&new));
    assert!(
        ids.iter()
            .enumerate()
            .all(|(i, id)| i == 1 || jobs.contains_key(id))
    );
}
