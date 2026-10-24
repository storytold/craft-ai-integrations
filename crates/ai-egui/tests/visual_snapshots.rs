//! Deterministic visual snapshots for the shared widgets.
//!
//! The fixtures are synthetic and local: no provider, download, keychain or clock read. egui runs
//! with its bundled test fonts, a fixed logical viewport, kittest's animations disabled and a fixed
//! step clock. The parent generates and reviews the PNGs with a managed CPU-rendered run; this file
//! intentionally carries no hand-made golden image.
//!
//! Each test also checks the semantics behind its picture: field names and roles at a narrow
//! viewport, visible and clickable Cancel buttons, and history row geometry at a larger font.

use craft_ai_core::history::{EndRecord, HISTORY_SCHEMA, StartRecord, merge};
use craft_ai_core::{HistoryLog, HistoryRecord, JobState, Locality, Outcome};
use craft_ai_egui::{
    CANCEL, CLEAR, EXPORT, FormEvent, HistoryAction, HistoryViewState, ProgressAction, REFRESH,
    Range, connection_form, history_view, job_progress,
};
use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder, SnapshotOptions};

const T0: u64 = 1_760_000_000_000; // 2025-10-09T08:53:20Z
const DARK_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(27, 27, 27);

/// One fixed renderer and clock for every image. `wgpu` is kittest's headless renderer; its default
/// setup prefers a CPU adapter so the parent can qualify the image without a physical GPU.
fn snapshot_builder<State>(size: egui::Vec2) -> HarnessBuilder<State> {
    Harness::builder()
        .wgpu()
        .with_size(size)
        .with_pixels_per_point(1.0)
        .with_os(egui::os::OperatingSystem::Nix)
        .with_theme(egui::Theme::Dark)
        .with_step_dt(0.25)
        .with_max_steps(4)
        .with_render_cursor(false)
        .with_context_setup(|ctx| {
            ctx.set_fonts(egui::FontDefinitions::default());
        })
}

fn snapshot_options() -> SnapshotOptions {
    SnapshotOptions::new()
        .output_path("tests/snapshots")
        .threshold(0.6)
        .max_failed_pixels(0)
}

fn screen(size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, size)
}

struct Job {
    run: &'static str,
    at: u64,
    app: &'static str,
    model: &'static str,
    outcome: Outcome,
}

fn job(run: &'static str, at: u64, model: &'static str) -> Job {
    Job {
        run,
        at,
        app: "photocraft",
        model,
        outcome: Outcome::Done,
    }
}

fn history_log(jobs: &[Job]) -> HistoryLog {
    let mut records = Vec::new();
    for job in jobs {
        records.push(HistoryRecord::Start(StartRecord {
            v: HISTORY_SCHEMA,
            run: job.run.into(),
            at_ms: job.at,
            app: job.app.into(),
            role_id: "fill".into(),
            role_label: "Generative Fill".into(),
            capability: "image.inpaint".into(),
            connection: "hosted".into(),
            connection_label: "Hosted".into(),
            model: job.model.into(),
            locality: Locality::Remote,
            destination: "https://api.example.com".into(),
            sent: Vec::new(),
            params: Vec::new(),
            provider_job: None,
            estimate: None,
            prompt: None,
        }));
        records.push(HistoryRecord::End(EndRecord {
            v: HISTORY_SCHEMA,
            run: job.run.into(),
            at_ms: job.at + 1000,
            outcome: job.outcome.clone(),
            usage: Default::default(),
        }));
    }
    merge(records, 0)
}

// ---- history: a larger font and a narrow viewport ----

struct HistoryState {
    view: HistoryViewState,
    log: HistoryLog,
    now: u64,
    row_height: f32,
    spacing: f32,
    action: Option<HistoryAction>,
}

fn large_font_history() -> Harness<'static, HistoryState> {
    let jobs: Vec<Job> = (0..40u64)
        .map(|i| {
            let run: &'static str = Box::leak(format!("run-{i}").into_boxed_str());
            let model: &'static str = Box::leak(format!("model-{i:02}").into_boxed_str());
            job(run, T0 + i * 60_000, model)
        })
        .collect();
    snapshot_builder(egui::vec2(480.0, 700.0)).build_ui_state(
        |ui, state: &mut HistoryState| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, DARK_BACKGROUND);
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(26.0));
            state.row_height = ui.text_style_height(&egui::TextStyle::Body);
            state.spacing = ui.spacing().item_spacing.y;
            if let Some(action) = history_view(ui, &mut state.view, &state.log, state.now) {
                state.action = Some(action);
            }
        },
        HistoryState {
            view: HistoryViewState::default(),
            log: history_log(&jobs),
            now: T0 + 40 * 60_000,
            row_height: 0.0,
            spacing: 0.0,
            action: None,
        },
    )
}

fn visible_rows(h: &Harness<'_, HistoryState>) -> Vec<(usize, egui::Rect)> {
    let mut rows: Vec<(usize, egui::Rect)> = h
        .query_all_by_label_contains("photocraft")
        .filter_map(|node| {
            let accessibility = node.accesskit_node();
            let label = accessibility.value().or_else(|| accessibility.label())?;
            let part = label.split("  \u{b7}  ").nth(3)?;
            let model = part.split(' ').next()?;
            let index = model.strip_prefix("model-")?.parse().ok()?;
            Some((index, node.rect()))
        })
        .collect();
    rows.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    rows
}

#[test]
fn history_at_a_larger_font_uses_one_text_height_per_row() {
    let mut h = large_font_history();
    h.run();
    let rows = visible_rows(&h);
    assert!(rows.len() > 2, "the large-font fixture has visible rows");
    assert_eq!(rows.first().unwrap().0, 39, "newest job first");
    let expected = h.state().row_height + h.state().spacing;
    assert!(expected > 20.0, "the fixture really uses a larger font");
    for pair in rows.windows(2) {
        assert_eq!(pair[0].0, pair[1].0 + 1, "history order is preserved");
        let delta = pair[1].1.top() - pair[0].1.top();
        assert!(
            (delta - expected).abs() < 1.5,
            "row spacing is {delta}, expected {expected}"
        );
    }
    h.snapshot_options("history_large_font_dark_480x700", &snapshot_options());
}

fn narrow_history() -> Harness<'static, HistoryState> {
    let jobs = [
        job("a", T0, "flux"),
        job("b", T0 + 1000, "gpt-4o"),
        job("c", T0 + 2000, "flux"),
    ];
    let log = history_log(&jobs);
    let mut view = HistoryViewState::default();
    view.range = Range::All;
    snapshot_builder(egui::vec2(240.0, 420.0)).build_ui_state(
        move |ui, state: &mut HistoryState| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, DARK_BACKGROUND);
            state.row_height = ui.text_style_height(&egui::TextStyle::Body);
            state.spacing = ui.spacing().item_spacing.y;
            if let Some(action) = history_view(ui, &mut state.view, &state.log, state.now) {
                state.action = Some(action);
            }
        },
        HistoryState {
            view,
            log,
            now: T0 + 5000,
            row_height: 0.0,
            spacing: 0.0,
            action: None,
        },
    )
}

#[test]
fn history_filters_and_actions_stay_inside_a_narrow_viewport() {
    let mut h = narrow_history();
    h.run();
    assert_eq!(
        h.query_all_by_label_contains("photocraft").count(),
        3,
        "all three synthetic jobs are shown"
    );
    let viewport = screen(egui::vec2(240.0, 420.0));
    for label in ["Filter by app", "Filter by model", "Date range"] {
        let node = h.get_by_label(label);
        assert!(
            viewport.contains_rect(node.rect()),
            "{label} is outside {viewport:?}: {:?}",
            node.rect()
        );
    }
    for label in [REFRESH, EXPORT, CLEAR] {
        let node = h.get_by_label(label);
        assert!(
            viewport.contains_rect(node.rect()),
            "{label} is outside {viewport:?}: {:?}",
            node.rect()
        );
    }
    assert!(h.unnamed_widgets().is_empty());
    h.snapshot_options("history_narrow_dark_240x420", &snapshot_options());
    h.get_by_label(REFRESH).click();
    h.run();
    assert_eq!(h.state().action, Some(HistoryAction::Refresh));
}

// ---- progress: Cancel in both layout directions ----

struct ProgressState {
    job: JobState,
    cancelled: bool,
}

fn progress_harness(right_to_left: bool) -> Harness<'static, ProgressState> {
    let size = egui::vec2(320.0, 480.0);
    snapshot_builder(size).build_ui_state(
        move |ui, state: &mut ProgressState| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, DARK_BACKGROUND);
            let draw = |ui: &mut egui::Ui, state: &mut ProgressState| {
                if job_progress(ui, &state.job, None) == Some(ProgressAction::Cancel) {
                    state.cancelled = true;
                }
            };
            if right_to_left {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    draw(ui, state);
                });
            } else {
                draw(ui, state);
            }
        },
        ProgressState {
            job: JobState::Running { percent: Some(40) },
            cancelled: false,
        },
    )
}

fn cancel_bounds_are_usable(h: &Harness<'_, ProgressState>, right_to_left: bool) {
    let viewport = screen(egui::vec2(320.0, 480.0));
    let bar = h.get_by_label_contains("Working, 40%");
    let cancel = h.get_by_label(CANCEL);
    assert!(
        bar.rect().top() < 40.0 && cancel.rect().top() < 40.0,
        "the progress row must remain at the top in either direction: bar {:?}, cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    assert!(viewport.contains_rect(bar.rect()));
    assert!(viewport.contains_rect(cancel.rect()));
    assert!(
        !bar.rect().intersects(cancel.rect()),
        "bar {:?} overlaps Cancel {:?}",
        bar.rect(),
        cancel.rect()
    );
    if right_to_left {
        assert!(
            cancel.rect().right() <= bar.rect().left() + 0.5,
            "right-to-left order puts Cancel before the bar: bar {:?}, cancel {:?}",
            bar.rect(),
            cancel.rect()
        );
    } else {
        assert!(
            bar.rect().right() <= cancel.rect().left() + 0.5,
            "left-to-right order puts Cancel after the bar: bar {:?}, cancel {:?}",
            bar.rect(),
            cancel.rect()
        );
    }
}

#[test]
fn progress_cancel_is_visible_and_clickable_left_to_right() {
    let mut h = progress_harness(false);
    h.run();
    cancel_bounds_are_usable(&h, false);
    h.snapshot_options("progress_cancel_ltr_dark_320x480", &snapshot_options());
    h.get_by_label(CANCEL).click();
    h.run();
    assert!(h.state().cancelled);
}

#[test]
fn progress_cancel_is_visible_and_clickable_right_to_left() {
    let mut h = progress_harness(true);
    h.run();
    cancel_bounds_are_usable(&h, true);
    h.snapshot_options("progress_cancel_rtl_dark_320x480", &snapshot_options());
    h.get_by_label(CANCEL).click();
    h.run();
    assert!(h.state().cancelled);
}

// ---- connection form: names, roles and bounds at a narrow viewport ----

struct FormState {
    form: craft_ai_egui::ConnectionFormState,
    event: Option<FormEvent>,
}

fn connection_harness() -> Harness<'static, FormState> {
    snapshot_builder(egui::vec2(240.0, 560.0)).build_ui_state(
        |ui, state: &mut FormState| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, DARK_BACKGROUND);
            if let Some(event) = connection_form(ui, &mut state.form, false) {
                state.event = Some(event);
            }
        },
        FormState {
            form: craft_ai_egui::ConnectionFormState::new(),
            event: None,
        },
    )
}

#[test]
fn connection_fields_keep_their_names_roles_and_bounds_when_narrow() {
    let mut h = connection_harness();
    h.run();
    let viewport = screen(egui::vec2(240.0, 560.0));
    let fields: Vec<(Role, Option<String>, egui::Rect)> = h
        .get_all_by_role(Role::TextInput)
        .map(|node| {
            let accessibility = node.accesskit_node();
            (accessibility.role(), accessibility.label(), node.rect())
        })
        .collect();
    assert_eq!(
        fields
            .iter()
            .map(|(role, label, _)| (*role, label.clone()))
            .collect::<Vec<_>>(),
        [
            (Role::TextInput, Some("Name".to_owned())),
            (Role::TextInput, Some("Address".to_owned())),
        ]
    );
    for (_, label, rect) in &fields {
        assert!(
            viewport.contains_rect(*rect),
            "{label:?} is outside {viewport:?}: {rect:?}"
        );
    }
    let key = h.get_by_role(Role::PasswordInput);
    assert_eq!(key.accesskit_node().label().as_deref(), Some("Key"));
    assert!(viewport.contains_rect(key.rect()));
    assert!(h.unnamed_widgets().is_empty());
    for label in ["Uses a key", "Save", "Test", "Cancel"] {
        let node = h.get_by_label(label);
        assert!(
            viewport.contains_rect(node.rect()),
            "{label} is outside {viewport:?}: {:?}",
            node.rect()
        );
    }

    h.snapshot_options("connection_fields_narrow_dark_240x560", &snapshot_options());
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().event, Some(FormEvent::Save));
}
