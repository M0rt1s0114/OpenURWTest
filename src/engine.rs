//! Write / verify engine.
//!
//! One pass writes test data until the target volume has less than one block
//! of free space left, then reads it back and compares every block against the
//! deterministic payload. Multiple passes repeat the cycle; test data is
//! removed between passes and kept after the last one.
//!
//! The engine does not print anything itself: progress and results are sent to
//! a Reporter, and a per-block timing trace is returned for charts/analysis.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cancel::{self, CancelToken};
use crate::format::{self, BLOCK_SIZE, BLOCKS_PER_FILE, MIN_FREE_SPACE, Prng};
use crate::health::{self, TemperatureSample};
use crate::io::AlignedBlock;
use crate::pause::PauseToken;
use crate::reporter::{ErrorInfo, ProgressInfo, Reporter};
use crate::trace::{Phase, RunTrace};
use crate::volume;

const MIB: f64 = 1024.0 * 1024.0;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(512);

/// Options for a write/verify run.
#[derive(Debug, Clone)]
pub struct RunOptions<'a> {
    pub target: &'a Path,
    pub passes: u32,
    pub verify: bool,
    pub continue_on_error: bool,
    pub reuse_existing: bool,
    pub cancel: Option<CancelToken>,
    pub pause: Option<PauseToken>,
}

fn wait_if_paused(pause: Option<&PauseToken>, cancel: Option<&CancelToken>) -> Duration {
    let Some(pause) = pause else {
        return Duration::ZERO;
    };
    let mut paused = Duration::ZERO;
    while pause.is_paused() && !is_cancelled(cancel) {
        let started = Instant::now();
        thread::sleep(Duration::from_millis(20));
        paused += started.elapsed();
    }
    paused
}

fn is_cancelled(cancel: Option<&CancelToken>) -> bool {
    cancel.is_some_and(CancelToken::is_cancelled) || cancel::is_cancelled()
}

/// Per-phase counters.
#[derive(Debug, Default, Clone, Copy)]
pub struct PhaseStats {
    pub bytes: u64,
    pub seconds: f64,
    pub data_errors: u64,
    pub read_errors: u64,
    pub write_errors: u64,
    pub interrupted: bool,
    pub device_lost: bool,
}

impl PhaseStats {
    pub fn failed(&self) -> bool {
        self.data_errors > 0 || self.read_errors > 0 || self.write_errors > 0
    }

    pub fn mib_per_sec(&self) -> f64 {
        if self.seconds <= 0.0 {
            0.0
        } else {
            self.bytes as f64 / self.seconds / MIB
        }
    }
}

/// Result of a whole run.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub passes: u32,
    pub passed: u32,
    pub failed: u32,
    pub error_pass: Option<u32>,
    pub wrote_bytes: u64,
    pub read_bytes: u64,
    pub data_errors: u64,
    pub read_errors: u64,
    pub write_errors: u64,
    pub interrupted: bool,
    pub device_lost: bool,
    pub write_only: bool,
    pub trace: RunTrace,
    pub temperature_history: Vec<TemperatureSample>,
}

impl Summary {
    #[cfg(test)]
    pub fn success(&self) -> bool {
        self.failed == 0 && self.error_pass.is_none() && self.write_errors == 0
    }
}

/// Source of free-space information, abstracted so the engine can be tested
/// without touching a real volume.
pub trait SpaceProbe {
    fn free_bytes(&self, target: &Path) -> Result<u64>;
}

pub struct OsSpace;

impl SpaceProbe for OsSpace {
    fn free_bytes(&self, target: &Path) -> Result<u64> {
        volume::free_space(target)
    }
}

pub fn run_os(options: &RunOptions<'_>, reporter: &mut dyn Reporter) -> Result<Summary> {
    run(options, &OsSpace, reporter)
}

pub fn run(
    options: &RunOptions<'_>,
    space: &dyn SpaceProbe,
    reporter: &mut dyn Reporter,
) -> Result<Summary> {
    let passes = options.passes.max(1);
    let mut summary = Summary {
        passes,
        ..Summary::default()
    };
    let run_start = Instant::now();
    sample_temperature(options.target, &run_start, &mut summary.temperature_history);

    reporter.run_started(options.target, passes, options.verify);

    for pass in 1..=passes {
        reporter.pass_started(pass, passes);

        let existing = format::scan_test_files(options.target)?;
        if !existing.is_empty() && options.reuse_existing {
            reporter.info("Existing test data found; verifying it directly.");
        } else {
            if !existing.is_empty() {
                reporter.info("Existing test data found; removing it before a fresh write.");
                format::delete_test_files(options.target)?;
            }
            let stats = write_pass(
                options.target,
                pass,
                passes,
                space,
                reporter,
                options.cancel.as_ref(),
                options.pause.as_ref(),
                &mut summary.trace,
                &mut summary.temperature_history,
                run_start,
            )?;
            summary.wrote_bytes += stats.bytes;
            summary.write_errors += stats.write_errors;
            summary.interrupted |= stats.interrupted;
            summary.device_lost |= stats.device_lost;

            if stats.interrupted || stats.device_lost || stats.write_errors > 0 {
                if stats.write_errors > 0 {
                    summary.failed += 1;
                    summary.error_pass = Some(pass);
                }
                reporter.info("Write stopped; test data kept on the volume.");
                reporter.finished(&summary);
                return Ok(summary);
            }

            if !options.verify {
                summary.write_only = true;
                reporter.finished(&summary);
                return Ok(summary);
            }
        }

        let stats = verify_pass(
            options.target,
            pass,
            passes,
            reporter,
            options.cancel.as_ref(),
            options.pause.as_ref(),
            &mut summary.trace,
            &mut summary.temperature_history,
            run_start,
        )?;
        summary.read_bytes += stats.bytes;
        summary.data_errors += stats.data_errors;
        summary.read_errors += stats.read_errors;
        summary.interrupted |= stats.interrupted;
        summary.device_lost |= stats.device_lost;

        if stats.interrupted {
            if stats.failed() {
                summary.failed += 1;
            } else {
                summary.passed += 1;
            }
            reporter.finished(&summary);
            return Ok(summary);
        }

        if stats.failed() {
            summary.failed += 1;
            if pass < passes {
                if options.continue_on_error {
                    reporter.warn("Verification errors; removing test data and continuing.");
                    format::delete_test_files(options.target)?;
                    continue;
                }
                summary.error_pass = Some(pass);
                break;
            }
        } else {
            summary.passed += 1;
        }

        if pass < passes {
            reporter.info("Removing test data before the next pass.");
            format::delete_test_files(options.target)?;
        }
    }

    reporter.finished(&summary);
    Ok(summary)
}

/// Verify test data that already exists on the target.
pub fn verify_existing(target: &Path, reporter: &mut dyn Reporter) -> Result<Summary> {
    let mut summary = Summary {
        passes: 1,
        ..Summary::default()
    };
    let run_start = Instant::now();

    reporter.run_started(target, 1, true);
    reporter.pass_started(1, 1);

    let stats = verify_pass(
        target,
        1,
        1,
        reporter,
        None,
        None,
        &mut summary.trace,
        &mut Vec::new(),
        run_start,
    )?;
    summary.read_bytes = stats.bytes;
    summary.data_errors = stats.data_errors;
    summary.read_errors = stats.read_errors;
    summary.interrupted = stats.interrupted;
    summary.device_lost = stats.device_lost;

    if stats.failed() {
        summary.failed = 1;
    } else {
        summary.passed = 1;
    }

    reporter.finished(&summary);
    Ok(summary)
}

struct PhaseProgress {
    pass: u32,
    passes: u32,
    phase: Phase,
    total: u64,
    start: Instant,
    last_tick: Instant,
    last_bytes: u64,
}

impl PhaseProgress {
    fn new(pass: u32, passes: u32, phase: Phase, total: u64) -> Self {
        let now = Instant::now();
        Self {
            pass,
            passes,
            phase,
            total,
            start: now,
            last_tick: now,
            last_bytes: 0,
        }
    }

    fn skip_pause(&mut self, paused: Duration) {
        if !paused.is_zero() {
            self.start += paused;
            self.last_tick += paused;
        }
    }

    fn tick(&mut self, reporter: &mut dyn Reporter, bytes: u64) {
        if self.last_tick.elapsed() < PROGRESS_INTERVAL {
            return;
        }
        let dt = self.last_tick.elapsed().as_secs_f64().max(0.001);
        let elapsed = self.start.elapsed();
        let current = (bytes.saturating_sub(self.last_bytes)) as f64 / dt / MIB;
        let average = bytes as f64 / elapsed.as_secs_f64().max(0.001) / MIB;
        let remaining = self.total.saturating_sub(bytes);
        let eta = if average > 0.01 {
            (remaining as f64 / MIB / average) as u64
        } else {
            0
        };
        reporter.progress(&ProgressInfo {
            pass: self.pass,
            passes: self.passes,
            phase: self.phase,
            bytes,
            total_bytes: self.total,
            current_mibps: current,
            average_mibps: average,
            remaining_bytes: remaining,
            eta_secs: eta,
        });
        self.last_tick = Instant::now();
        self.last_bytes = bytes;
    }
}

#[allow(clippy::too_many_arguments)]
fn write_pass(
    target: &Path,
    pass: u32,
    passes: u32,
    space: &dyn SpaceProbe,
    reporter: &mut dyn Reporter,
    cancel: Option<&CancelToken>,
    pause: Option<&PauseToken>,
    trace: &mut RunTrace,
    temperature_history: &mut Vec<TemperatureSample>,
    run_start: Instant,
) -> Result<PhaseStats> {
    let start_free = space.free_bytes(target)?;
    if start_free <= MIN_FREE_SPACE {
        bail!(
            "not enough free space on {} ({} bytes available)",
            target.display(),
            start_free
        );
    }

    let phase_start = Instant::now();
    let phase_offset = run_start.elapsed().as_millis() as u64;
    trace.record(pass, Phase::Write, 0, 0, phase_offset);
    reporter.phase_started(pass, Phase::Write, start_free);

    let mut buffer = AlignedBlock::new(BLOCK_SIZE as usize);
    let mut prng = Prng::new();
    let mut blocks = 0u64;
    let mut file: Option<File> = None;
    let mut interrupted = false;
    let mut device_lost = false;
    let mut write_errors = 0u64;
    let mut clock = PhaseProgress::new(pass, passes, Phase::Write, start_free);
    let mut paused_total = Duration::ZERO;

    loop {
        let paused = wait_if_paused(pause, cancel);
        paused_total += paused;
        clock.skip_pause(paused);
        let free = space.free_bytes(target)?;
        if free <= MIN_FREE_SPACE {
            break;
        }

        if blocks.is_multiple_of(BLOCKS_PER_FILE) {
            let index = (blocks / BLOCKS_PER_FILE + 1) as u32;
            let path = target.join(format::test_file_name(index));
            file = Some(
                crate::io::open_writer(&path)
                    .with_context(|| format!("failed to create {}", path.display()))?,
            );
        }

        prng.fill_block(buffer.as_mut_slice());
        if let Some(file) = file.as_mut()
            && let Err(error) = file.write_all(buffer.as_slice())
        {
            if is_disk_full(&error) {
                break;
            }
            if is_device_lost(&error) {
                device_lost = true;
                break;
            }
            write_errors += 1;
            reporter.error(&ErrorInfo {
                pass,
                phase: Phase::Write,
                block: blocks,
                message: "write error",
            });
            break;
        }
        blocks += 1;

        let phase_ms = phase_start
            .elapsed()
            .saturating_sub(paused_total)
            .as_millis() as u64;
        trace.record(
            pass,
            Phase::Write,
            blocks * BLOCK_SIZE,
            phase_ms,
            phase_offset + phase_ms,
        );

        if is_cancelled(cancel) {
            interrupted = true;
            break;
        }

        clock.tick(reporter, blocks * BLOCK_SIZE);
        sample_temperature(target, &run_start, temperature_history);
    }

    drop(file);
    let stats = PhaseStats {
        bytes: blocks * BLOCK_SIZE,
        seconds: phase_start
            .elapsed()
            .saturating_sub(paused_total)
            .as_secs_f64(),
        interrupted,
        device_lost,
        write_errors,
        ..PhaseStats::default()
    };
    reporter.phase_finished(pass, Phase::Write, &stats);
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn verify_pass(
    target: &Path,
    pass: u32,
    passes: u32,
    reporter: &mut dyn Reporter,
    cancel: Option<&CancelToken>,
    pause: Option<&PauseToken>,
    trace: &mut RunTrace,
    temperature_history: &mut Vec<TemperatureSample>,
    run_start: Instant,
) -> Result<PhaseStats> {
    let files = format::scan_test_files(target)?;
    if files.is_empty() {
        bail!("no test data found on {}", target.display());
    }
    let total = format::total_blocks(&files)
        .ok_or_else(|| anyhow::anyhow!("test data on {} is empty", target.display()))?;
    let total_bytes = total * BLOCK_SIZE;

    let phase_start = Instant::now();
    let phase_offset = run_start.elapsed().as_millis() as u64;
    trace.record(pass, Phase::Verify, 0, 0, phase_offset);
    reporter.phase_started(pass, Phase::Verify, total_bytes);

    let mut expected = AlignedBlock::new(BLOCK_SIZE as usize);
    let mut actual = AlignedBlock::new(BLOCK_SIZE as usize);
    let mut prng = Prng::new();
    let mut file: Option<File> = None;
    let mut stats = PhaseStats::default();
    let mut verified = 0u64;
    let mut clock = PhaseProgress::new(pass, passes, Phase::Verify, total_bytes);
    let mut paused_total = Duration::ZERO;

    for block in 0..total {
        let paused = wait_if_paused(pause, cancel);
        paused_total += paused;
        clock.skip_pause(paused);
        if block.is_multiple_of(BLOCKS_PER_FILE) {
            let path = format::block_path(target, block);
            file = Some(
                crate::io::open_reader(&path)
                    .with_context(|| format!("failed to open {}", path.display()))?,
            );
        }

        let reader = file.as_mut().expect("file opened above");
        let result = reader.read_exact(actual.as_mut_slice());
        match result {
            Ok(()) => {
                prng.fill_block(expected.as_mut_slice());
                if expected.as_slice() != actual.as_slice() {
                    stats.data_errors += 1;
                    reporter.error(&ErrorInfo {
                        pass,
                        phase: Phase::Verify,
                        block,
                        message: "data error",
                    });
                }
            }
            Err(error) => {
                stats.read_errors += 1;
                reporter.error(&ErrorInfo {
                    pass,
                    phase: Phase::Verify,
                    block,
                    message: "read error",
                });
                if is_device_lost(&error) {
                    stats.device_lost = true;
                    break;
                }
                prng.skip_block();
                let in_file = block % BLOCKS_PER_FILE;
                let _ = reader.seek(SeekFrom::Start((in_file + 1) * BLOCK_SIZE));
            }
        }

        verified += 1;
        let phase_ms = phase_start
            .elapsed()
            .saturating_sub(paused_total)
            .as_millis() as u64;
        trace.record(
            pass,
            Phase::Verify,
            verified * BLOCK_SIZE,
            phase_ms,
            phase_offset + phase_ms,
        );

        if is_cancelled(cancel) {
            stats.interrupted = true;
            break;
        }

        clock.tick(reporter, verified * BLOCK_SIZE);
        sample_temperature(target, &run_start, temperature_history);
    }

    stats.bytes = verified * BLOCK_SIZE;
    stats.seconds = phase_start.elapsed().as_secs_f64();
    reporter.phase_finished(pass, Phase::Verify, &stats);
    Ok(stats)
}

const TEMPERATURE_INTERVAL_MS: u64 = 30_000;

fn sample_temperature(target: &Path, run_start: &Instant, history: &mut Vec<TemperatureSample>) {
    let elapsed_ms = run_start.elapsed().as_millis() as u64;
    if history
        .last()
        .is_some_and(|last| elapsed_ms.saturating_sub(last.elapsed_ms) < TEMPERATURE_INTERVAL_MS)
    {
        return;
    }
    if let Some(temperature_c) = health::read_health(target).temperature_c {
        history.push(TemperatureSample {
            elapsed_ms,
            temperature_c,
        });
    }
}

fn is_disk_full(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(39) | Some(112))
    }
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(28)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = error;
        false
    }
}

fn is_device_lost(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(21) | Some(433) | Some(1167))
    }
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(19)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = error;
        false
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use super::*;
    use crate::reporter::NullReporter;

    fn test_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("urwtest-rs-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create temp dir");
        directory
    }

    fn options(
        target: &Path,
        passes: u32,
        verify: bool,
        continue_on_error: bool,
    ) -> RunOptions<'_> {
        RunOptions {
            target,
            passes,
            verify,
            continue_on_error,
            reuse_existing: true,
            cancel: None,
            pause: None,
        }
    }

    struct FakeSpace {
        capacity: u64,
        calls: Cell<u64>,
    }

    impl FakeSpace {
        fn new(capacity: u64) -> Self {
            Self {
                capacity,
                calls: Cell::new(0),
            }
        }
    }

    impl SpaceProbe for FakeSpace {
        fn free_bytes(&self, target: &Path) -> Result<u64> {
            let empty = crate::format::scan_test_files(target)
                .map(|files| files.is_empty())
                .unwrap_or(true);
            if empty {
                self.calls.set(0);
            }
            let used = self.calls.get() * BLOCK_SIZE;
            self.calls.set(self.calls.get() + 1);
            Ok(self.capacity.saturating_sub(used))
        }
    }

    fn write_blocks(directory: &Path, blocks: u64, corrupt: Option<u64>) {
        let mut prng = Prng::new();
        let mut buffer = AlignedBlock::new(BLOCK_SIZE as usize);
        let mut file = None;
        for block in 0..blocks {
            if block.is_multiple_of(BLOCKS_PER_FILE) {
                let path =
                    directory.join(format::test_file_name((block / BLOCKS_PER_FILE + 1) as u32));
                file = Some(crate::io::open_writer(&path).expect("create test file"));
            }
            prng.fill_block(buffer.as_mut_slice());
            if Some(block) == corrupt {
                buffer.as_mut_slice()[0] ^= 0xFF;
            }
            file.as_mut()
                .expect("file opened")
                .write_all(buffer.as_slice())
                .expect("write block");
        }
    }

    #[test]
    fn write_pass_stops_at_free_space_threshold() {
        let directory = test_dir("write-pass");
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        let stats = write_pass(
            &directory,
            1,
            1,
            &space,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("write pass");
        assert!(!stats.interrupted);
        assert_eq!(stats.bytes, 3 * BLOCK_SIZE);
        let files = format::scan_test_files(&directory).expect("scan");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].size, 3 * BLOCK_SIZE);
        assert!(trace.points.iter().any(|point| point.phase == Phase::Write));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn verify_pass_accepts_valid_data() {
        let directory = test_dir("verify-ok");
        write_blocks(&directory, 3, None);
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        let stats = verify_pass(
            &directory,
            1,
            1,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("verify pass");
        assert_eq!(stats.data_errors, 0);
        assert_eq!(stats.read_errors, 0);
        assert_eq!(stats.bytes, 3 * BLOCK_SIZE);
        assert!(
            trace
                .points
                .iter()
                .any(|point| point.phase == Phase::Verify)
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn verify_pass_detects_corruption() {
        let directory = test_dir("verify-bad");
        write_blocks(&directory, 3, Some(1));
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        let stats = verify_pass(
            &directory,
            1,
            1,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("verify pass");
        assert_eq!(stats.data_errors, 1);
        assert_eq!(stats.read_errors, 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn run_without_verify_keeps_data_and_reports_write_only() {
        let directory = test_dir("run-write-only");
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let summary =
            run(&options(&directory, 1, false, false), &space, &mut reporter).expect("run");
        assert!(summary.write_only);
        assert_eq!(summary.wrote_bytes, 3 * BLOCK_SIZE);
        let files = format::scan_test_files(&directory).expect("scan");
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn run_keeps_data_after_last_pass_and_removes_between_passes() {
        let directory = test_dir("run-passes");
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let summary =
            run(&options(&directory, 2, true, false), &space, &mut reporter).expect("run");
        assert_eq!(summary.passed, 2);
        assert!(summary.success());
        let files = format::scan_test_files(&directory).expect("scan");
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn run_stops_early_and_keeps_data_after_failure() {
        let directory = test_dir("run-stop");
        write_blocks(&directory, 3, Some(1));
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let summary =
            run(&options(&directory, 2, true, false), &space, &mut reporter).expect("run");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.error_pass, Some(1));
        let files = format::scan_test_files(&directory).expect("scan");
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn run_continues_after_failure_when_requested() {
        let directory = test_dir("run-continue");
        write_blocks(&directory, 3, Some(1));
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let summary = run(&options(&directory, 2, true, true), &space, &mut reporter).expect("run");
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.error_pass, None);
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        let stats = verify_pass(
            &directory,
            2,
            2,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("verify kept data");
        assert!(!stats.failed());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn verify_existing_reports_success_and_failure() {
        let directory = test_dir("verify-existing");
        write_blocks(&directory, 2, None);
        let mut reporter = NullReporter;
        let good = verify_existing(&directory, &mut reporter).expect("verify good");
        assert_eq!(good.passed, 1);
        assert_eq!(good.failed, 0);
        let _ = std::fs::remove_dir_all(&directory);

        std::fs::create_dir_all(&directory).expect("recreate temp dir");
        write_blocks(&directory, 2, Some(0));
        let mut reporter = NullReporter;
        let bad = verify_existing(&directory, &mut reporter).expect("verify bad");
        assert_eq!(bad.passed, 0);
        assert_eq!(bad.failed, 1);
        assert_eq!(bad.data_errors, 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn write_pass_stops_when_cancelled() {
        let directory = test_dir("write-cancel");
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        crate::cancel::reset();
        crate::cancel::request_for_test();
        let stats = write_pass(
            &directory,
            1,
            1,
            &space,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("write pass");
        crate::cancel::reset();
        assert!(stats.interrupted);
        assert_eq!(stats.bytes, BLOCK_SIZE);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn verify_pass_stops_when_cancelled() {
        let directory = test_dir("verify-cancel");
        write_blocks(&directory, 3, None);
        let mut reporter = NullReporter;
        let mut trace = RunTrace::default();
        crate::cancel::reset();
        crate::cancel::request_for_test();
        let stats = verify_pass(
            &directory,
            1,
            1,
            &mut reporter,
            None,
            None,
            &mut trace,
            &mut Vec::new(),
            Instant::now(),
        )
        .expect("verify pass");
        crate::cancel::reset();
        assert!(stats.interrupted);
        assert_eq!(stats.bytes, BLOCK_SIZE);
        assert!(!stats.failed());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(windows)]
    #[test]
    fn disk_full_error_codes_are_recognised() {
        assert!(is_disk_full(&io::Error::from_raw_os_error(39)));
        assert!(is_disk_full(&io::Error::from_raw_os_error(112)));
        assert!(!is_disk_full(&io::Error::from_raw_os_error(5)));
    }

    #[cfg(unix)]
    #[test]
    fn disk_full_error_codes_are_recognised() {
        assert!(is_disk_full(&io::Error::from_raw_os_error(28)));
        assert!(!is_disk_full(&io::Error::from_raw_os_error(5)));
    }

    #[cfg(windows)]
    #[test]
    fn device_lost_error_codes_are_recognised() {
        assert!(is_device_lost(&io::Error::from_raw_os_error(433)));
        assert!(is_device_lost(&io::Error::from_raw_os_error(1167)));
        assert!(!is_device_lost(&io::Error::from_raw_os_error(5)));
        assert!(!is_disk_full(&io::Error::from_raw_os_error(433)));
    }

    #[test]
    fn trace_positions_increase_per_phase() {
        let directory = test_dir("trace");
        let space = FakeSpace::new(4 * BLOCK_SIZE);
        let mut reporter = NullReporter;
        let summary =
            run(&options(&directory, 1, true, false), &space, &mut reporter).expect("run");
        let write_points: Vec<_> = summary
            .trace
            .points
            .iter()
            .filter(|point| point.phase == Phase::Write)
            .collect();
        let verify_points: Vec<_> = summary
            .trace
            .points
            .iter()
            .filter(|point| point.phase == Phase::Verify)
            .collect();
        assert!(!write_points.is_empty());
        assert!(!verify_points.is_empty());
        for pair in write_points.windows(2) {
            assert!(pair[1].position >= pair[0].position);
        }
        for pair in verify_points.windows(2) {
            assert!(pair[1].position >= pair[0].position);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn paused_run_resumes_after_token() {
        let directory = test_dir("pause");
        let directory_for_thread = directory.clone();
        let pause = PauseToken::new();
        pause.pause();
        let pause_for_thread = pause.clone();
        let handle = thread::spawn(move || {
            let mut reporter = NullReporter;
            let options = RunOptions {
                target: &directory_for_thread,
                passes: 1,
                verify: true,
                continue_on_error: false,
                reuse_existing: false,
                cancel: None,
                pause: Some(pause_for_thread),
            };
            let space = FakeSpace::new(8 * BLOCK_SIZE);
            (run(&options, &space, &mut reporter), directory_for_thread)
        });
        thread::sleep(Duration::from_millis(40));
        assert!(!handle.is_finished());
        pause.resume();
        let (result, directory) = handle.join().expect("pause thread");
        assert_eq!(result.expect("run").passed, 1);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
