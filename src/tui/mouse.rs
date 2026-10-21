//! Mouse capture and hotspot actions.

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyCode};
use ratatui::layout::Rect;

use super::SettingsField;

pub(super) struct MouseGuard {
    pub(super) enabled: bool,
}

impl MouseGuard {
    pub(super) fn new(enabled: bool) -> Self {
        if enabled {
            let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        }
        Self { enabled }
    }

    pub(super) fn set(&mut self, enabled: bool) -> Result<()> {
        if enabled == self.enabled {
            return Ok(());
        }
        if enabled {
            crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
        } else {
            crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
        }
        self.enabled = enabled;
        Ok(())
    }
}

impl Drop for MouseGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseAction {
    Select(usize),
    Toggle(usize),
    Key(KeyCode),
    FocusSettings(SettingsField),
    FocusSpeed(usize),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Hotspot {
    pub(super) rect: Rect,
    pub(super) action: MouseAction,
}
