//! The widgets driven headlessly: real egui layout, real clicks, no window and no GPU.

use craft_ai_client::{Choice, RoleReady, RoleStatus};
use craft_ai_core::consent::DisclosureItem;
use craft_ai_core::{
    AiError, AppId, Billing, CapabilityId, ConnectionId, CostEstimate, Disclosure, ErrorKind,
    InputRole, JobState, Locality, ModelId, PayloadKind,
};
use craft_ai_egui::{
    ADD_CONNECTION, ALLOW, CANCEL, ConsentChoice, ConsentPromptState, DENY, OPEN_LAUNCHER,
    ProgressAction, StatusAction, StatusView, TimedConsentChoice, USE_DEFAULT, ai_status,
    consent_prompt, consent_prompt_with_terms, consent_term_label, job_progress,
};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn disclosure(locality: Locality) -> Disclosure {
    Disclosure {
        app: AppId::new("photocraft").unwrap(),
        connection: ConnectionId::new("cloud").unwrap(),
        capability: CapabilityId::parse("image.inpaint").unwrap(),
        locality,
        destination: "https://api.example.com".into(),
        items: vec![DisclosureItem {
            role: InputRole::Source,
            kind: PayloadKind::Image,
            bytes: Some(2_500_000),
        }],
        param_names: vec!["size".into()],
    }
}

fn choice(conn: &str, model: &str, label: &str, locality: Locality) -> Choice {
    Choice {
        connection: ConnectionId::new(conn).unwrap(),
        connection_label: format!("Conn {conn}"),
        model: ModelId::new(model).unwrap(),
        model_label: label.into(),
        locality,
    }
}

fn ready(conn: &str, model: &str, label: &str) -> RoleStatus {
    RoleStatus::Ready(RoleReady {
        connection: ConnectionId::new(conn).unwrap(),
        connection_label: format!("Conn {conn}"),
        model: ModelId::new(model).unwrap(),
        model_label: label.into(),
        locality: Locality::Local,
        billing: Billing::Free,
    })
}

// ---- consent ----

fn consent_harness(d: Disclosure) -> Harness<'static, Option<ConsentChoice>> {
    Harness::new_ui_state(
        move |ui, state: &mut Option<ConsentChoice>| {
            if let Some(c) = consent_prompt(ui, &d, "PhotoCraft") {
                *state = Some(c);
            }
        },
        None,
    )
}

#[test]
fn consent_shows_destination_kinds_sizes_and_the_remote_badge() {
    let mut h = consent_harness(disclosure(Locality::Remote));
    h.run();
    h.get_by_label("Leaves this computer");
    h.get_by_label("PhotoCraft will send the following to https://api.example.com");
    h.get_by_label("\u{2022} image (source), 2.5 MB");
    h.get_by_label("\u{2022} settings: size");
    assert_eq!(
        *h.state(),
        None,
        "nothing is chosen until a button is pressed"
    );
}

#[test]
fn consent_allow_and_deny_report_exactly_one_choice() {
    let mut h = consent_harness(disclosure(Locality::Remote));
    h.run();
    h.get_by_label(ALLOW).click();
    h.run();
    assert_eq!(*h.state(), Some(ConsentChoice::Allow));

    let mut h = consent_harness(disclosure(Locality::Remote));
    h.run();
    h.get_by_label(DENY).click();
    h.run();
    assert_eq!(*h.state(), Some(ConsentChoice::Deny));
}

#[test]
fn consent_for_a_local_model_says_it_stays_here() {
    let mut h = consent_harness(disclosure(Locality::Local));
    h.run();
    h.get_by_label("Stays on this computer");
    assert!(h.query_by_label("Leaves this computer").is_none());
}

// ---- status ----

struct StatusState {
    status: RoleStatus,
    choices: Vec<Choice>,
    has_override: bool,
    can_add_here: bool,
    last: Option<StatusAction>,
}

fn status_harness(s: StatusState) -> Harness<'static, StatusState> {
    Harness::new_ui_state(
        |ui, st: &mut StatusState| {
            let view = StatusView {
                label: "Generative Fill",
                status: &st.status,
                choices: &st.choices,
                has_override: st.has_override,
                can_add_here: st.can_add_here,
            };
            if let Some(a) = ai_status(ui, &view) {
                st.last = Some(a);
            }
        },
        s,
    )
}

fn status_harness_sized(size: egui::Vec2, s: StatusState) -> Harness<'static, StatusState> {
    Harness::builder().with_size(size).build_ui_state(
        |ui, st: &mut StatusState| {
            let view = StatusView {
                label: "Generative Fill",
                status: &st.status,
                choices: &st.choices,
                has_override: st.has_override,
                can_add_here: st.can_add_here,
            };
            if let Some(a) = ai_status(ui, &view) {
                st.last = Some(a);
            }
        },
        s,
    )
}

#[test]
fn unavailable_feature_explains_itself_and_offers_the_launcher() {
    let mut h = status_harness(StatusState {
        status: RoleStatus::Unavailable {
            reason: "no model is chosen for 'fill'".into(),
        },
        choices: vec![],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    h.get_by_label("Generative Fill is off: no model is chosen for 'fill'");
    assert!(
        h.query_by_label(USE_DEFAULT).is_none(),
        "no override, no reset button"
    );
    h.get_by_label(OPEN_LAUNCHER).click();
    h.run();
    assert_eq!(h.state().last, Some(StatusAction::OpenLauncher));
}

#[test]
fn ready_feature_shows_its_model_and_no_launcher_button() {
    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![choice("local", "llama3", "Llama 3", Locality::Local)],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    h.get_by_label("Generative Fill: Llama 3 on Conn local (stays on this computer, free)");
    assert!(h.query_by_label(OPEN_LAUNCHER).is_none());
}

#[test]
fn picking_another_model_reports_it_and_the_current_one_does_not() {
    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![
            choice("local", "llama3", "Llama 3", Locality::Local),
            choice("cloud", "gpt-4o", "GPT-4o", Locality::Remote),
        ],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    h.get_by_role(Role::ComboBox).click();
    h.run();
    // Choosing what is already selected changes nothing.
    h.query_all_by_label_contains("Llama 3 \u{2014} Conn local")
        .last()
        .unwrap()
        .click();
    h.run();
    assert_eq!(h.state().last, None);

    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![
            choice("local", "llama3", "Llama 3", Locality::Local),
            choice("cloud", "gpt-4o", "GPT-4o", Locality::Remote),
        ],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label_contains("GPT-4o \u{2014} Conn cloud (leaves this computer)")
        .click();
    h.run();
    assert_eq!(
        h.state().last,
        Some(StatusAction::Pick {
            connection: ConnectionId::new("cloud").unwrap(),
            model: ModelId::new("gpt-4o").unwrap(),
        })
    );
}

#[test]
fn use_default_appears_only_with_an_override() {
    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![],
        has_override: true,
        can_add_here: false,
        last: None,
    });
    h.run();
    h.get_by_label(USE_DEFAULT).click();
    h.run();
    assert_eq!(h.state().last, Some(StatusAction::UseDefault));
}

#[test]
fn adding_a_connection_here_is_offered_only_when_asked_and_only_when_unavailable() {
    let unavailable = || RoleStatus::Unavailable {
        reason: "no connection".into(),
    };
    // Off by default: the launcher is the only way.
    let mut h = status_harness(StatusState {
        status: unavailable(),
        choices: vec![],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    assert!(h.query_by_label(ADD_CONNECTION).is_none());
    h.get_by_label(OPEN_LAUNCHER);
    // On: offered next to the launcher button, not instead of it.
    let mut h = status_harness(StatusState {
        status: unavailable(),
        choices: vec![],
        has_override: false,
        can_add_here: true,
        last: None,
    });
    h.run();
    h.get_by_label(OPEN_LAUNCHER);
    h.get_by_label(ADD_CONNECTION).click();
    h.run();
    assert_eq!(h.state().last, Some(StatusAction::AddConnection));
    // A feature that already works has nothing to add.
    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![],
        has_override: false,
        can_add_here: true,
        last: None,
    });
    h.run();
    assert!(h.query_by_label(ADD_CONNECTION).is_none());
}

// ---- progress ----

struct ProgressState {
    job: JobState,
    cost: Option<CostEstimate>,
    cancelled: bool,
}

fn progress_harness(job: JobState, cost: Option<CostEstimate>) -> Harness<'static, ProgressState> {
    Harness::new_ui_state(
        |ui, st: &mut ProgressState| {
            if job_progress(ui, &st.job, st.cost.as_ref()) == Some(ProgressAction::Cancel) {
                st.cancelled = true;
            }
        },
        ProgressState {
            job,
            cost,
            cancelled: false,
        },
    )
}

fn progress_harness_sized(
    size: egui::Vec2,
    job: JobState,
    cost: Option<CostEstimate>,
) -> Harness<'static, ProgressState> {
    Harness::builder().with_size(size).build_ui_state(
        |ui, st: &mut ProgressState| {
            if job_progress(ui, &st.job, st.cost.as_ref()) == Some(ProgressAction::Cancel) {
                st.cancelled = true;
            }
        },
        ProgressState {
            job,
            cost,
            cancelled: false,
        },
    )
}

/// The same widget under a right-to-left host layout, at a narrow width.
fn progress_harness_rtl_sized(
    size: egui::Vec2,
    job: JobState,
    cost: Option<CostEstimate>,
) -> Harness<'static, ProgressState> {
    Harness::builder().with_size(size).build_ui_state(
        |ui, st: &mut ProgressState| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if job_progress(ui, &st.job, st.cost.as_ref()) == Some(ProgressAction::Cancel) {
                    st.cancelled = true;
                }
            });
        },
        ProgressState {
            job,
            cost,
            cancelled: false,
        },
    )
}

#[test]
fn a_running_job_shows_percent_cost_and_can_be_cancelled() {
    let cost = CostEstimate {
        micros: 40_000,
        currency: "USD".into(),
        billing: Billing::Metered,
    };
    let mut h = progress_harness(JobState::Running { percent: Some(40) }, Some(cost));
    h.run();
    h.get_by_label_contains("Working, 40%");
    h.get_by_label("about 0.04 USD (billed per use)");
    h.get_by_label(CANCEL).click();
    h.run();
    assert!(h.state().cancelled);
}

#[test]
fn a_job_with_no_percentage_still_shows_it_is_working() {
    let mut h = progress_harness(JobState::Running { percent: None }, None);
    // A spinner repaints forever, so step a few frames instead of running to rest.
    h.step();
    h.step();
    h.get_by_label("Working");
    h.get_by_label(CANCEL);
}

#[test]
fn finished_jobs_say_how_they_ended_and_cannot_be_cancelled() {
    for (state, text) in [
        (JobState::Succeeded, "Done".to_owned()),
        (JobState::Cancelled, "Cancelled".to_owned()),
        (
            JobState::Failed {
                error: AiError::new(ErrorKind::Quota, "out of credits"),
            },
            "Failed: out of credits".to_owned(),
        ),
    ] {
        let mut h = progress_harness(state, None);
        h.run();
        h.get_by_label_contains(&text);
        assert!(h.query_by_label(CANCEL).is_none(), "{text}");
    }
}

#[test]
fn timed_consent_radios_expose_their_selected_state_and_duration() {
    struct State {
        prompt: ConsentPromptState,
        choice: Option<TimedConsentChoice>,
    }
    let d = disclosure(Locality::Remote);
    let mut h = Harness::new_ui_state(
        move |ui, state: &mut State| {
            if let Some(choice) = consent_prompt_with_terms(ui, &d, "PhotoCraft", &mut state.prompt)
            {
                state.choice = Some(choice);
            }
        },
        State {
            prompt: ConsentPromptState::default(),
            choice: None,
        },
    );
    h.run();
    let this_time = h.get_by_label(consent_term_label(craft_ai_core::ConsentTerm::ThisTime));
    assert_eq!(this_time.accesskit_node().role(), Role::RadioButton);
    assert_eq!(
        this_time.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    let one_hour = h.get_by_label(consent_term_label(craft_ai_core::ConsentTerm::OneHour));
    assert_eq!(
        one_hour.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::False)
    );
    one_hour.click();
    h.run();
    assert_eq!(
        h.get_by_label(consent_term_label(craft_ai_core::ConsentTerm::OneHour))
            .accesskit_node()
            .toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    h.get_by_label(ALLOW).click();
    h.run();
    assert_eq!(
        h.state().choice,
        Some(TimedConsentChoice::Allow(
            craft_ai_core::ConsentTerm::OneHour
        ))
    );
}

#[test]
fn the_status_model_picker_has_a_field_name_not_just_its_value() {
    let mut h = status_harness(StatusState {
        status: ready("local", "llama3", "Llama 3"),
        choices: vec![
            choice("local", "llama3", "Llama 3", Locality::Local),
            choice("cloud", "gpt-4o", "GPT-4o", Locality::Remote),
        ],
        has_override: false,
        can_add_here: false,
        last: None,
    });
    h.run();
    let combo = h.get_by_role(Role::ComboBox);
    assert_eq!(
        combo.accesskit_node().label().as_deref(),
        Some("Model for Generative Fill")
    );
    assert!(h.unnamed_widgets().is_empty());
}

#[test]
fn status_commands_stay_inside_a_narrow_header() {
    let mut h = status_harness_sized(
        egui::vec2(180.0, 360.0),
        StatusState {
            status: RoleStatus::Unavailable {
                reason: "no connection".into(),
            },
            choices: vec![],
            has_override: false,
            can_add_here: true,
            last: None,
        },
    );
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 360.0));
    for label in [OPEN_LAUNCHER, ADD_CONNECTION] {
        let node = h.get_by_label(label);
        assert!(
            screen.contains_rect(node.rect()),
            "{label} is outside {screen:?}: {:?}",
            node.rect()
        );
    }
    h.get_by_label(ADD_CONNECTION).click();
    h.run();
    assert_eq!(h.state().last, Some(StatusAction::AddConnection));
}

#[test]
fn cancel_stays_visible_and_does_not_overlap_progress_at_320() {
    let mut h = progress_harness_sized(
        egui::vec2(320.0, 480.0),
        JobState::Running { percent: Some(40) },
        None,
    );
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 480.0));
    let bar = h.get_by_label_contains("Working, 40%");
    let cancel = h.get_by_label(CANCEL);
    assert!(screen.contains_rect(bar.rect()));
    assert!(screen.contains_rect(cancel.rect()));
    assert!(
        !bar.rect().intersects(cancel.rect()),
        "bar {:?} overlaps Cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    cancel.click();
    h.run();
    assert!(h.state().cancelled);
}

#[test]
fn narrow_cancel_wraps_below_the_bar_and_stays_clickable() {
    let mut h = progress_harness_sized(
        egui::vec2(120.0, 480.0),
        JobState::Running { percent: Some(40) },
        None,
    );
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 480.0));
    let bar = h.get_by_label_contains("Working, 40%");
    let cancel = h.get_by_label(CANCEL);
    assert!(screen.contains_rect(cancel.rect()), "Cancel is clipped");
    assert!(
        cancel.rect().top() >= bar.rect().bottom(),
        "Cancel should wrap below the bar: bar {:?}, cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    cancel.click();
    h.run();
    assert!(h.state().cancelled);
}

#[test]
fn narrow_spinner_job_also_keeps_cancel_reachable() {
    let mut h = progress_harness_sized(
        egui::vec2(120.0, 480.0),
        JobState::Running { percent: None },
        None,
    );
    h.step();
    h.step();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 480.0));
    let cancel = h.get_by_label(CANCEL);
    assert!(screen.contains_rect(cancel.rect()));
    cancel.click();
    // The spinner keeps requesting repaints, so one step is enough to see the click.
    h.step();
    assert!(h.state().cancelled);
}

#[test]
fn rtl_narrow_progress_keeps_cancel_visible_and_apart_from_the_bar() {
    let mut h = progress_harness_rtl_sized(
        egui::vec2(320.0, 480.0),
        JobState::Running { percent: Some(40) },
        None,
    );
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 480.0));
    let bar = h.get_by_label_contains("Working, 40%");
    let cancel = h.get_by_label(CANCEL);
    assert!(screen.contains_rect(bar.rect()));
    assert!(screen.contains_rect(cancel.rect()));
    assert!(
        !bar.rect().intersects(cancel.rect()),
        "bar {:?} overlaps Cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    assert!(
        cancel.rect().right() <= bar.rect().left() + 0.5,
        "right-to-left order puts Cancel before the bar: bar {:?}, cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    cancel.click();
    h.run();
    assert!(h.state().cancelled);
}

#[test]
fn rtl_narrow_spinner_job_keeps_cancel_visible_and_clickable() {
    let mut h = progress_harness_rtl_sized(
        egui::vec2(320.0, 480.0),
        JobState::Running { percent: None },
        None,
    );
    // The spinner keeps asking for repaints, so step instead of running to rest.
    h.step();
    h.step();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 480.0));
    let label = h.get_by_label("Working");
    let cancel = h.get_by_label(CANCEL);
    assert!(screen.contains_rect(label.rect()));
    assert!(screen.contains_rect(cancel.rect()));
    assert!(
        !label.rect().intersects(cancel.rect()),
        "label {:?} overlaps Cancel {:?}",
        label.rect(),
        cancel.rect()
    );
    assert!(
        cancel.rect().right() <= label.rect().left() + 0.5,
        "right-to-left order puts Cancel before the label: label {:?}, cancel {:?}",
        label.rect(),
        cancel.rect()
    );
    cancel.click();
    h.step();
    assert!(h.state().cancelled);
}

#[test]
fn the_fixed_fixture_glyphs_are_in_the_test_fonts() {
    let mut h = Harness::new_ui_state(
        |ui, _| {
            ui.label("\u{2022} bullet \u{2026} ellipsis \u{2014} dash \u{00b7} middle dot");
        },
        (),
    );
    // The harness panics on a missing glyph, which is the assertion here.
    h.run();
    assert!(h.unnamed_widgets().is_empty());
}
