//! Jobs: a worker thread per request, and the shared cell the provider polls.
//!
//! The [`Provider`](craft_ai_core::Provider) contract is submit, poll, cancel. Each submit
//! validates and builds the request on the caller's thread (so a bad request fails before any
//! network use), then hands it to a worker thread that talks to the server and writes into a
//! [`JobCell`]. Polls read the cell.
//!
//! Cancelling marks the job `Cancelled` at once and discards whatever the worker produces later. A
//! streamed reply is also closed at the next line, which ends the server's work. A reply that is
//! not streamed cannot be interrupted mid-flight, so its worker finishes and is discarded; the
//! request timeout bounds how long that takes.

use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use craft_ai_core::job::{Event, JobSnapshot, JobState};
use craft_ai_core::{AiError, ErrorKind, JobId, Usage};

use crate::chat::{self, ChatPlan};
use crate::http::Http;
use crate::images::{self, ImagePlan};
use crate::stream::{MAX_SSE_LINE, SseState};

/// Finished jobs kept for polling before the oldest are forgotten.
const KEEP_FINISHED: usize = 64;

/// Most events one job keeps while waiting to be polled. A stream that produces more than
/// this without a poll fails rather than growing the job's memory without end.
const MAX_EVENTS: usize = 65_536;

/// A request ready to run.
pub(crate) enum Plan {
    Chat(ChatPlan),
    Image(ImagePlan),
}

struct Inner {
    state: JobState,
    events: Vec<Event>,
    usage: Option<Usage>,
}

/// One job's shared state. Terminal states are final: later writes are ignored.
pub(crate) struct JobCell {
    inner: Mutex<Inner>,
}

impl JobCell {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: JobState::Queued,
                events: Vec::new(),
                usage: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_terminal(&self) -> bool {
        self.lock().state.is_terminal()
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().state == JobState::Cancelled
    }

    fn start(&self) {
        let mut g = self.lock();
        if g.state == JobState::Queued {
            g.state = JobState::Running { percent: None };
        }
    }

    /// Adds an event for the next poll; a backlog that passes [`MAX_EVENTS`] fails the job
    /// rather than grows while nobody reads it.
    pub fn push(&self, event: Event) {
        let mut g = self.lock();
        if g.state.is_terminal() {
            return;
        }
        // A backlog no poll ever drains is a hostile stream (or a job nobody reads); refuse
        // the rest rather than grow it for as long as the request timeout allows.
        if g.events.len() >= MAX_EVENTS {
            g.state = JobState::Failed {
                error: AiError::new(
                    ErrorKind::Provider,
                    "too many events are waiting; poll the job while it runs",
                ),
            };
            g.events.clear();
            return;
        }
        g.events.push(event);
    }

    /// Marks the job Succeeded with its usage, unless it already ended.
    fn succeed(&self, usage: Option<Usage>) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Succeeded;
            g.usage = usage;
        }
    }

    fn fail(&self, error: AiError) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Failed { error };
        }
    }

    pub fn cancel(&self) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Cancelled;
            g.events.clear();
        }
    }

    /// The state now and the events since the last call.
    pub fn snapshot(&self) -> JobSnapshot {
        let mut g = self.lock();
        JobSnapshot {
            state: g.state.clone(),
            events: std::mem::take(&mut g.events),
            usage: g.usage.clone(),
        }
    }
}

/// All jobs of one provider.
pub(crate) struct Registry {
    next: AtomicU64,
    max_active: usize,
    jobs: Mutex<Vec<(JobId, Arc<JobCell>)>>,
}

impl Registry {
    pub fn new(max_active: usize) -> Self {
        Self {
            next: AtomicU64::new(1),
            max_active: max_active.max(1),
            jobs: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(JobId, Arc<JobCell>)>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self, id: &JobId) -> Option<Arc<JobCell>> {
        self.lock()
            .iter()
            .find(|(j, _)| j == id)
            .map(|(_, c)| Arc::clone(c))
    }

    /// Starts a worker for `plan`.
    pub fn spawn(&self, http: Arc<Http>, plan: Plan) -> Result<JobId, AiError> {
        let cell = Arc::new(JobCell::new());
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let id = JobId::new(format!("openai-{n}"))
            .map_err(|e| AiError::new(ErrorKind::Internal, e.to_string()))?;
        {
            let mut jobs = self.lock();
            if jobs.iter().filter(|(_, c)| !c.is_terminal()).count() >= self.max_active {
                return Err(AiError::new(
                    ErrorKind::RateLimited,
                    "too many jobs are running; wait for one to finish",
                ));
            }
            jobs.push((id.clone(), Arc::clone(&cell)));
            while jobs.len() > KEEP_FINISHED {
                match jobs.iter().position(|(_, c)| c.is_terminal()) {
                    Some(i) => {
                        jobs.remove(i);
                    }
                    None => break,
                }
            }
        }
        let worker_cell = Arc::clone(&cell);
        let spawned = std::thread::Builder::new()
            .name(format!("craft-ai-{id}"))
            .spawn(move || run(&http, &worker_cell, plan));
        if spawned.is_err() {
            self.lock().retain(|(j, _)| j != &id);
            return Err(AiError::new(
                ErrorKind::Internal,
                "could not start a worker thread",
            ));
        }
        Ok(id)
    }
}

fn run(http: &Http, cell: &JobCell, plan: Plan) {
    cell.start();
    let outcome = match plan {
        Plan::Chat(p) => run_chat(http, cell, &p),
        Plan::Image(p) => run_image(http, cell, &p),
    };
    match outcome {
        Ok(usage) => cell.succeed(http.trust_cost(usage)),
        Err(e) => cell.fail(http.scrub(e)),
    }
}

fn run_image(http: &Http, cell: &JobCell, p: &ImagePlan) -> Result<Option<Usage>, AiError> {
    let resp = http.post(p.path, &p.content_type, &p.body)?;
    let v = http.read_json(resp)?;
    if cell.is_cancelled() {
        return Ok(None);
    }
    let (outputs, usage) = images::parse_images(&v, p.fallback_media)?;
    for o in outputs {
        cell.push(Event::Output(o));
    }
    Ok(usage)
}

/// Posts one chat request, whole or streamed, and turns the reply into events.
fn run_chat(http: &Http, cell: &JobCell, p: &ChatPlan) -> Result<Option<Usage>, AiError> {
    let body = serde_json::to_vec(&p.body)
        .map_err(|_| AiError::new(ErrorKind::Internal, "could not encode the request"))?;
    let resp = http.post("/chat/completions", "application/json", &body)?;
    if cell.is_cancelled() {
        return Ok(None);
    }
    if !p.stream {
        let completion = chat::parse_completion(&http.read_json(resp)?)?;
        for event in chat::events_for(&completion, p.structured)? {
            cell.push(event);
        }
        return Ok(completion.usage);
    }
    let mut reader = BufReader::new(resp.into_body().into_reader());
    let mut state = SseState::default();
    let mut line = Vec::new();
    let cap = u64::try_from(MAX_SSE_LINE)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    loop {
        // Dropping the reader on return closes the connection, which stops the server's work.
        // Mid-stream a terminal cell means the job was cancelled, or its event backlog grew
        // past the cap and it failed; either way nothing more will ever be read from it.
        if cell.is_terminal() {
            return Ok(None);
        }
        line.clear();
        let n = (&mut reader)
            .take(cap)
            .read_until(b'\n', &mut line)
            .map_err(|_| {
                AiError::new(ErrorKind::Network, "the connection broke while streaming")
            })?;
        if n == 0 {
            break;
        }
        if line.len() > MAX_SSE_LINE {
            return Err(AiError::new(
                ErrorKind::Provider,
                "the stream held a line that is too long",
            ));
        }
        let delta = state.line(&String::from_utf8_lossy(&line))?;
        if let Some(text) = delta.filter(|_| !p.structured) {
            cell.push(Event::TextDelta { text });
        }
        if state.done {
            break;
        }
    }
    if !state.done && !state.finished {
        return Err(AiError::new(
            ErrorKind::Network,
            "the stream ended before the reply was complete",
        ));
    }
    let completion = state.into_completion();
    if p.structured {
        for event in chat::events_for(&completion, true)? {
            cell.push(event);
        }
    } else {
        for (name, arguments) in &completion.tool_calls {
            cell.push(Event::ToolCall {
                name: name.clone(),
                arguments: arguments.clone(),
            });
        }
    }
    Ok(completion.usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Event {
        Event::TextDelta { text: s.into() }
    }

    #[test]
    fn events_are_delivered_once() {
        let c = JobCell::new();
        c.start();
        c.push(text("a"));
        assert_eq!(c.snapshot().events.len(), 1);
        assert!(c.snapshot().events.is_empty());
        c.push(text("b"));
        c.succeed(None);
        let last = c.snapshot();
        assert_eq!(last.state, JobState::Succeeded);
        assert_eq!(last.events.len(), 1);
        assert!(c.snapshot().events.is_empty());
    }

    #[test]
    fn terminal_states_are_final() {
        let c = JobCell::new();
        c.start();
        c.cancel();
        c.push(text("late"));
        c.succeed(None);
        c.fail(AiError::new(ErrorKind::Provider, "late"));
        let s = c.snapshot();
        assert_eq!(s.state, JobState::Cancelled);
        assert!(s.events.is_empty());
        assert!(c.is_cancelled());

        let f = JobCell::new();
        f.fail(AiError::new(ErrorKind::Provider, "x"));
        f.cancel();
        f.succeed(None);
        assert!(matches!(f.snapshot().state, JobState::Failed { .. }));
    }

    /// `start` moves a Queued job to Running once; later starts change nothing.
    #[test]
    fn start_moves_queued_to_running_once() {
        let c = JobCell::new();
        assert_eq!(c.snapshot().state, JobState::Queued);
        c.start();
        assert_eq!(c.snapshot().state, JobState::Running { percent: None });
        c.succeed(None);
        c.start();
        assert_eq!(c.snapshot().state, JobState::Succeeded);
    }

    /// A backlog past the cap fails the job and is dropped; one at the cap survives a poll.
    #[test]
    fn an_event_backlog_past_the_cap_fails_the_job() {
        // 65_536 is MAX_EVENTS, written out so that loosening the cap fails this test.
        // A snapshot drains the backlog, so the boundary and the refusal use two jobs.
        let boundary = JobCell::new();
        boundary.start();
        for _ in 0..65_536 {
            boundary.push(text("a"));
        }
        let kept = boundary.snapshot();
        assert_eq!(kept.events.len(), 65_536);
        assert_eq!(kept.state, JobState::Running { percent: None });
        let c = JobCell::new();
        c.start();
        for _ in 0..65_536 {
            c.push(text("a"));
        }
        // One event more is refused, not grown: the job fails and the backlog is dropped.
        c.push(text("one too many"));
        let failed = c.snapshot();
        assert!(matches!(failed.state, JobState::Failed { .. }));
        assert!(failed.events.is_empty());
    }
}
