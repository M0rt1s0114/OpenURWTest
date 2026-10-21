//! Cooperative cancellation for Ctrl-C / Ctrl-Break.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

static CANCELLED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

#[cfg(test)]
thread_local! {
    static TEST_CANCELLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Install the platform control handler. Call once at startup.
pub fn install() -> anyhow::Result<()> {
    ctrlc::set_handler(|| CANCELLED.store(true, Ordering::SeqCst))
        .map_err(|error| anyhow::anyhow!("failed to install Ctrl-C handler: {error}"))
}

/// Shared flag for consumers that poll cancellation with an `Arc<AtomicBool>`.
pub fn flag() -> Arc<AtomicBool> {
    Arc::clone(&CANCELLED)
}

/// Request cancellation from the UI or another thread.
pub fn request() {
    CANCELLED.store(true, Ordering::SeqCst);
}

pub fn reset() {
    CANCELLED.store(false, Ordering::SeqCst);
    #[cfg(test)]
    TEST_CANCELLED.with(|value| value.set(false));
}

pub fn is_cancelled() -> bool {
    if CANCELLED.load(Ordering::SeqCst) {
        return true;
    }
    #[cfg(test)]
    {
        TEST_CANCELLED.with(|value| value.get())
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Request cancellation from a unit test without touching the process-wide flag.
#[cfg(test)]
pub fn request_for_test() {
    TEST_CANCELLED.with(|value| value.set(true));
}

/// Per-run cancellation token for concurrent tasks.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}
