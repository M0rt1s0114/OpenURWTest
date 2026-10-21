//! Engine output interface.
//!
//! The engine never prints directly. It reports progress and results through
//! this trait, so the same run can drive the line-based CLI output, the TUI, or
//! future machine-readable frontends.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::console::{Console, format_bytes};
use crate::engine::{PhaseStats, Summary};
use crate::terminal;
use crate::trace::Phase;
use crate::types::ProgressMode;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ProgressInfo {
    pub pass: u32,
    pub passes: u32,
    pub phase: Phase,
    pub bytes: u64,
    pub total_bytes: u64,
    pub current_mibps: f64,
    pub average_mibps: f64,
    pub remaining_bytes: u64,
    pub eta_secs: u64,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ErrorInfo<'a> {
    pub pass: u32,
    pub phase: Phase,
    pub block: u64,
    pub message: &'a str,
}

pub trait Reporter {
    fn run_started(&mut self, _target: &Path, _passes: u32, _verify: bool) {}
    fn pass_started(&mut self, _pass: u32, _passes: u32) {}
    fn info(&mut self, _message: &str) {}
    fn warn(&mut self, _message: &str) {}
    fn error(&mut self, _error: &ErrorInfo<'_>) {}
    fn phase_started(&mut self, _pass: u32, _phase: Phase, _total_bytes: u64) {}
    fn progress(&mut self, _progress: &ProgressInfo) {}
    fn phase_finished(&mut self, _pass: u32, _phase: Phase, _stats: &PhaseStats) {}
    fn finished(&mut self, _summary: &Summary) {}
}

/// Reporter that discards everything (used by unit tests).
#[allow(dead_code)]
pub struct NullReporter;

impl Reporter for NullReporter {}

/// Line-based reporter used by the CLI and the interactive text mode.
pub struct LineReporter<'a> {
    console: &'a Console,
    mode: ProgressMode,
    progress_started: Instant,
    last_output: Instant,
    last_bytes: u64,
    live_started: bool,
    errors_reported: usize,
    errors_suppressed: usize,
}

const MAX_REPORTED_ERRORS: usize = 20;

impl<'a> LineReporter<'a> {
    pub fn new(console: &'a Console, mode: ProgressMode) -> Self {
        let now = Instant::now();
        Self {
            console,
            mode,
            progress_started: now,
            last_output: now,
            last_bytes: 0,
            live_started: false,
            errors_reported: 0,
            errors_suppressed: 0,
        }
    }

    fn reset_progress(&mut self) {
        self.progress_started = Instant::now();
        self.last_output = Instant::now();
        self.last_bytes = 0;
        self.live_started = false;
    }
}

impl Reporter for LineReporter<'_> {
    fn pass_started(&mut self, pass: u32, passes: u32) {
        if passes > 1 {
            println!();
            println!("=== Pass {pass}/{passes} ===");
        }
    }

    fn info(&mut self, message: &str) {
        self.console.info(message);
    }

    fn warn(&mut self, message: &str) {
        self.console.warn(message);
    }

    fn error(&mut self, error: &ErrorInfo<'_>) {
        if self.errors_reported < MAX_REPORTED_ERRORS {
            let offset = error.block.saturating_mul(crate::format::BLOCK_SIZE);
            self.console.fail(&format!(
                "{} error at pass {} block {} ({}): {}",
                error.phase.label(),
                error.pass,
                error.block,
                format_bytes(offset),
                error.message
            ));
            self.errors_reported += 1;
        } else {
            self.errors_suppressed += 1;
        }
    }

    fn phase_started(&mut self, _pass: u32, _phase: Phase, _total_bytes: u64) {
        self.reset_progress();
    }

    fn progress(&mut self, progress: &ProgressInfo) {
        let interval = match self.mode {
            ProgressMode::Live => Duration::from_millis(512),
            ProgressMode::Plain => Duration::from_secs(10),
            ProgressMode::Auto | ProgressMode::Off => return,
        };
        if !self.live_started {
            self.last_output = Instant::now();
            self.live_started = true;
        }
        if self.last_output.elapsed() < interval {
            return;
        }

        let remaining = progress.remaining_bytes;
        match self.mode {
            ProgressMode::Live => {
                let eta = progress.eta_secs;
                let (hours, minutes, seconds) = (eta / 3600, (eta % 3600) / 60, eta % 60);
                let full = format!(
                    "{:<9} {:>8} | now {:>7.1} MiB/s | avg {:>7.1} MiB/s | left {:>8} | ETA {:02}:{:02}:{:02}",
                    progress.phase.label(),
                    format_bytes(progress.bytes),
                    progress.current_mibps,
                    progress.average_mibps,
                    format_bytes(remaining),
                    hours,
                    minutes,
                    seconds
                );
                let width = terminal::width().unwrap_or(100).saturating_sub(1).max(20);
                let line = if full.chars().count() <= width {
                    full
                } else {
                    let compact = format!(
                        "{:<9} {:>8} | {:>7.1} MiB/s | left {:>8}",
                        progress.phase.label(),
                        format_bytes(progress.bytes),
                        progress.average_mibps,
                        format_bytes(remaining)
                    );
                    if compact.chars().count() <= width {
                        compact
                    } else {
                        compact.chars().take(width).collect()
                    }
                };
                print!("\r{line}\x1b[K");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                terminal::mark_live_line();
            }
            ProgressMode::Plain => {
                println!(
                    "{}: {} | avg {:.1} MiB/s | left {}",
                    progress.phase.label(),
                    format_bytes(progress.bytes),
                    progress.average_mibps,
                    format_bytes(remaining)
                );
            }
            ProgressMode::Auto | ProgressMode::Off => {}
        }

        self.last_output = Instant::now();
        self.last_bytes = progress.bytes;
        let _ = self.progress_started;
    }

    fn phase_finished(&mut self, _pass: u32, phase: Phase, stats: &PhaseStats) {
        if self.live_started {
            terminal::finish_live_line();
            self.live_started = false;
        }
        let verb = match phase {
            Phase::Write => "Wrote",
            Phase::Verify => "Verified",
        };
        self.console.info(&format!(
            "{verb} {} in {:.1}s ({:.1} MiB/s).",
            format_bytes(stats.bytes),
            stats.seconds,
            stats.mib_per_sec()
        ));
        if self.errors_suppressed > 0 {
            self.console.warn(&format!(
                "{} additional error line(s) suppressed",
                self.errors_suppressed
            ));
        }
        self.errors_reported = 0;
        self.errors_suppressed = 0;
    }

    fn finished(&mut self, summary: &Summary) {
        if summary.write_only {
            self.console
                .info("Test data written; verification was skipped.");
        } else if summary.passed == 0 && summary.failed == 0 {
            if summary.interrupted {
                self.console.warn(
                    "Run was interrupted before any pass was verified; test data kept on the volume.",
                );
            } else if summary.device_lost {
                self.console.warn(
                    "The device disappeared before any pass was verified; results are incomplete.",
                );
            } else {
                self.console
                    .warn("Run did not complete; no pass was verified.");
            }
        } else {
            let details = format!(
                "{} pass(es): {} passed, {} failed, {} data error(s), {} read error(s)",
                summary.passes,
                summary.passed,
                summary.failed,
                summary.data_errors,
                summary.read_errors
            );
            if let Some(pass) = summary.error_pass {
                self.console.fail(&format!(
                    "{details}; stopped at pass {pass} after verification errors."
                ));
            } else if summary.failed == 0 {
                self.console.ok(&format!("{details}; all passes passed."));
            } else if summary.passed == 0 {
                self.console.fail(&format!("{details}; all passes failed."));
            } else {
                self.console
                    .fail(&format!("{details}; some passes failed."));
            }
        }

        if summary.interrupted && !(summary.passed == 0 && summary.failed == 0) {
            self.console
                .warn("Run was interrupted by the user; test data kept on the volume.");
        }
        if summary.device_lost && !(summary.passed == 0 && summary.failed == 0) {
            self.console
                .warn("The device disappeared during the run; results are incomplete.");
        }
    }
}
