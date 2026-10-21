//! Cooperative pause flag shared by the engine and TUI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default)]
pub struct PauseToken {
    paused: Arc<AtomicBool>,
}

impl PauseToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn toggle(&self) -> bool {
        let next = !self.paused.load(Ordering::SeqCst);
        self.paused.store(next, Ordering::SeqCst);
        next
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_and_resume_are_independent_per_token() {
        let first = PauseToken::new();
        let second = PauseToken::new();
        assert!(!first.is_paused());
        first.pause();
        assert!(first.is_paused());
        assert!(!second.is_paused());
        second.pause();
        first.resume();
        assert!(!first.is_paused());
        assert!(second.is_paused());
    }

    #[test]
    fn toggle_returns_new_state() {
        let token = PauseToken::new();
        assert!(token.toggle());
        assert!(token.is_paused());
        assert!(!token.toggle());
        assert!(!token.is_paused());
    }
}
