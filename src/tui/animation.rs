//! Activity indicator and animation state helpers.

use std::time::Duration;

use crate::types::TestKind;

use super::{App, Screen};

impl App {
    pub(super) fn animating(&self) -> bool {
        if self
            .error_flash_start
            .is_some_and(|start| start.elapsed() < Duration::from_millis(900))
        {
            return true;
        }
        match self.screen {
            Screen::Run => self.run.is_some(),
            Screen::MultiRun => self.multi.as_ref().is_some_and(|multi| !multi.all_done()),
            _ => false,
        }
    }

    pub(super) fn spinner_glyph(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let index = (self.animation_epoch.elapsed().as_millis() / 100) as usize % FRAMES.len();
        if self.ascii {
            const ASCII: [char; 4] = ['|', '/', '-', '\\'];
            ASCII[index % ASCII.len()]
        } else {
            FRAMES[index]
        }
    }

    pub(super) fn tab_activity_glyph(&self, kind: TestKind) -> &'static str {
        let index = (self.animation_epoch.elapsed().as_millis() / 100) as usize;
        if self.ascii {
            const ASCII: [&str; 4] = ["-", "\\", "|", "/"];
            return ASCII[index % ASCII.len()];
        }
        match kind {
            TestKind::Quality => {
                const FRAMES: [&str; 4] = ["◜", "◝", "◞", "◟"];
                FRAMES[index % FRAMES.len()]
            }
            TestKind::Speed => {
                const FRAMES: [&str; 6] = ["▁▃▅", "▃▅▇", "▅▇▅", "▇▅▃", "▅▃▁", "▃▁▃"];
                FRAMES[index % FRAMES.len()]
            }
        }
    }

    pub(super) fn error_pulse_on(&self) -> bool {
        self.error_flash_start.is_some_and(|start| {
            let elapsed = start.elapsed().as_millis();
            elapsed < 900 && (elapsed / 150) % 2 == 0
        })
    }
}
