//! Small shared types for a session's lifecycle: which prepared image a result belongs to, and
//! a handle that cancels only the request a session currently has in flight.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Which prepared image a session (or a result) refers to. Every `prepare`, failed `prepare`
/// and `release` starts a new revision, so a result whose revision is no longer current was
/// computed from a different image and must not be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

impl Revision {
    pub(crate) fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number itself; 0 means no image was ever prepared.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Cancels the request a session has in flight at the moment `cancel` is called, and nothing
/// started later. Cloneable and usable from any thread. It only ever touches that request's own
/// ComfyUI prompt, never the queue as a whole.
#[derive(Clone, Debug, Default)]
pub struct Sam3Canceller(Arc<AtomicU64>);

impl Sam3Canceller {
    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// A marker taken when a request starts.
    pub(crate) fn mark(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub(crate) fn fired_since(&self, mark: u64) -> bool {
        self.0.load(Ordering::SeqCst) != mark
    }
}
