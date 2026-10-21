//! CrystalDiskMark-style speed suite: write, read back, verify.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::health;
use crate::io::AlignedBlock;
use crate::speed::backend::{open_bench_file, prepare_file};
use crate::speed::metrics::{Counters, MetricsSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeedPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub block_size: u64,
    pub iodepth: u32,
    pub numjobs: u32,
    pub random: bool,
}

pub const PRESETS: [SpeedPreset; 4] = [
    SpeedPreset {
        id: "SEQ1M_Q8T1",
        name: "SEQ1M Q8T1",
        block_size: 1024 * 1024,
        iodepth: 8,
        numjobs: 1,
        random: false,
    },
    SpeedPreset {
        id: "SEQ1M_Q1T1",
        name: "SEQ1M Q1T1",
        block_size: 1024 * 1024,
        iodepth: 1,
        numjobs: 1,
        random: false,
    },
    SpeedPreset {
        id: "RND4K_Q32T1",
        name: "RND4K Q32T1",
        block_size: 4096,
        iodepth: 32,
        numjobs: 1,
        random: true,
    },
    SpeedPreset {
        id: "RND4K_Q1T1",
        name: "RND4K Q1T1",
        block_size: 4096,
        iodepth: 1,
        numjobs: 1,
        random: true,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuitePhase {
    Write,
    Verify,
}

impl SuitePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Verify => "read+verify",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SuiteOptions {
    pub file_size: u64,
    pub ignore_sequential_errors: bool,
    pub seed: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PresetResult {
    pub preset_index: usize,
    pub write: MetricsSnapshot,
    pub read: MetricsSnapshot,
    pub write_errors: u64,
    pub read_errors: u64,
    pub verify_errors: u64,
    pub stopped: bool,
    pub stopped_at: Option<u64>,
    pub write_curve: Vec<(u64, f64)>,
    pub read_curve: Vec<(u64, f64)>,
}

impl PresetResult {
    pub fn failed(&self) -> bool {
        self.write_errors > 0 || self.read_errors > 0 || self.verify_errors > 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct SuiteResult {
    pub presets: Vec<PresetResult>,
    pub file_size: u64,
    pub stopped: bool,
    pub cancelled: bool,
    pub stopped_at: Option<u64>,
    pub stopped_preset: Option<usize>,
    pub error: Option<String>,
    pub temperature_points: Vec<(u64, i32)>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SuiteEvent {
    PresetStarted {
        index: usize,
    },
    PhaseStarted {
        index: usize,
        phase: SuitePhase,
    },
    Progress {
        index: usize,
        phase: SuitePhase,
        done_bytes: u64,
        total_bytes: u64,
        mibps: f64,
    },
    Error {
        index: usize,
        phase: SuitePhase,
        offset: u64,
        kind: &'static str,
    },
    PresetFinished {
        index: usize,
        result: PresetResult,
    },
}

pub fn run_suite(
    target: &Path,
    presets: &[SpeedPreset],
    options: SuiteOptions,
    cancel: Arc<AtomicBool>,
    observer: &mut dyn FnMut(SuiteEvent),
) -> Result<SuiteResult> {
    if presets.is_empty() {
        bail!("no speed presets selected");
    }
    if options.file_size == 0 {
        bail!("speed test file size must be positive");
    }
    for preset in presets {
        if !options.file_size.is_multiple_of(preset.block_size) {
            bail!(
                "file size {} is not a multiple of block size {}",
                options.file_size,
                preset.block_size
            );
        }
        let workers = preset.numjobs as u64 * preset.iodepth as u64;
        if workers == 0 || workers > 256 {
            bail!("preset {} has invalid worker count {workers}", preset.name);
        }
    }

    let path = target.join("BENCH0001.DAT");
    let mut result = SuiteResult {
        file_size: options.file_size,
        ..SuiteResult::default()
    };
    let mut abort = false;
    let mut temperatures: Vec<(u64, i32)> = Vec::new();
    if let Some(value) = health::read_health(target).temperature_c {
        temperatures.push((0, value));
    }

    for preset in presets {
        let index = catalog_index(preset);
        if cancel.load(Ordering::SeqCst) || abort {
            break;
        }
        observer(SuiteEvent::PresetStarted { index });
        if let Err(error) = prepare_file(&path, options.file_size)
            .with_context(|| format!("failed to prepare benchmark file {}", path.display()))
        {
            result.error = Some(format!("{error:#}"));
            break;
        }

        let write = match run_phase(
            &path,
            preset,
            index,
            SuitePhase::Write,
            options.file_size,
            options.seed,
            &cancel,
            options.ignore_sequential_errors,
            observer,
        ) {
            Ok(write) => write,
            Err(error) => {
                result.error = Some(format!("{error:#}"));
                break;
            }
        };
        result
            .temperature_points
            .extend(write.temperature.iter().copied());
        let mut item = PresetResult {
            preset_index: index,
            write: write.metrics,
            write_curve: write.curve.clone(),
            write_errors: write.write_errors,
            stopped_at: write.stopped_at,
            stopped: write.stopped,
            ..PresetResult::default()
        };

        if write.stopped {
            result.stopped = true;
            result.stopped_at = write.stopped_at;
            result.stopped_preset = Some(index);
            observer(SuiteEvent::PresetFinished {
                index,
                result: item.clone(),
            });
            result.presets.push(item);
            break;
        }

        let read = match run_phase(
            &path,
            preset,
            index,
            SuitePhase::Verify,
            options.file_size,
            options.seed,
            &cancel,
            options.ignore_sequential_errors,
            observer,
        ) {
            Ok(read) => read,
            Err(error) => {
                result.error = Some(format!("{error:#}"));
                break;
            }
        };
        result
            .temperature_points
            .extend(read.temperature.iter().copied());
        item.read = read.metrics;
        item.read_curve = read.curve.clone();
        item.read_errors = read.read_errors;
        item.verify_errors = read.verify_errors;
        item.stopped = read.stopped;
        item.stopped_at = write.stopped_at.or(read.stopped_at);

        if read.stopped {
            abort = true;
            result.stopped = true;
            result.stopped_at = item.stopped_at;
            result.stopped_preset = Some(index);
        }
        result.presets.push(item.clone());
        observer(SuiteEvent::PresetFinished {
            index,
            result: item,
        });
        if abort {
            break;
        }
    }

    result.cancelled = cancel.load(Ordering::SeqCst);
    if let Some(value) = health::read_health(target).temperature_c {
        temperatures.push((result.file_size, value));
    }
    result.temperature_points = temperatures;
    Ok(result)
}

fn catalog_index(preset: &SpeedPreset) -> usize {
    PRESETS
        .iter()
        .position(|item| item.id == preset.id)
        .unwrap_or(0)
}

struct PhaseOutcome {
    metrics: MetricsSnapshot,
    curve: Vec<(u64, f64)>,
    temperature: Vec<(u64, i32)>,
    write_errors: u64,
    read_errors: u64,
    verify_errors: u64,
    stopped: bool,
    stopped_at: Option<u64>,
}

struct WorkerOutcome {
    counters: Counters,
    write_errors: u64,
    read_errors: u64,
    verify_errors: u64,
}

#[derive(Debug, Clone, Copy)]
struct WorkerError {
    offset: u64,
    kind: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn run_phase(
    path: &Path,
    preset: &SpeedPreset,
    index: usize,
    phase: SuitePhase,
    file_size: u64,
    seed: u64,
    cancel: &Arc<AtomicBool>,
    ignore_sequential_errors: bool,
    observer: &mut dyn FnMut(SuiteEvent),
) -> Result<PhaseOutcome> {
    let blocks = file_size / preset.block_size;
    anyhow::ensure!(blocks > 0, "speed test has no blocks");
    let workers = (preset.numjobs as u64 * preset.iodepth as u64).clamp(1, 256) as usize;
    let order = Arc::new(build_order(preset.random, blocks, seed, phase));
    let done = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (error_tx, error_rx) = mpsc::channel();
    let start = Instant::now();
    let strict = !preset.random && !ignore_sequential_errors;

    observer(SuiteEvent::PhaseStarted { index, phase });
    let mut handles = Vec::with_capacity(workers);
    for lane in 0..workers {
        let path = path.to_path_buf();
        let order = Arc::clone(&order);
        let done = Arc::clone(&done);
        let stop = Arc::clone(&stop);
        let cancel = Arc::clone(cancel);
        let error_tx = error_tx.clone();
        let block_size = preset.block_size;
        let is_read = phase == SuitePhase::Verify;
        handles.push(thread::spawn(move || {
            worker_loop(
                &path, block_size, is_read, lane, workers, &order, &done, &stop, &cancel, strict,
                &error_tx, seed,
            )
        }));
    }
    drop(error_tx);

    let mut last = Duration::ZERO;
    let mut last_bytes = 0u64;
    let mut errors_logged = 0u64;
    let mut first_error = None;
    let mut curve: Vec<(u64, f64)> = Vec::new();
    let mut temperature: Vec<(u64, i32)> = Vec::new();
    let mut next_temperature = Instant::now();
    loop {
        let finished = handles.iter().all(|handle| handle.is_finished());
        while let Ok(error) = error_rx.try_recv() {
            errors_logged += 1;
            if first_error.is_none() {
                first_error = Some(error.offset);
            }
            observer(SuiteEvent::Error {
                index,
                phase,
                offset: error.offset,
                kind: error.kind,
            });
        }
        let now = start.elapsed();
        if now.saturating_sub(last).as_millis() >= 200 || finished {
            let bytes = done
                .load(Ordering::Relaxed)
                .saturating_mul(preset.block_size);
            let interval = now.saturating_sub(last).as_secs_f64().max(0.001);
            let mibps = bytes.saturating_sub(last_bytes) as f64 / interval / (1024.0 * 1024.0);
            observer(SuiteEvent::Progress {
                index,
                phase,
                done_bytes: bytes,
                total_bytes: file_size,
                mibps,
            });
            curve.push((bytes, mibps));
            if next_temperature.elapsed() >= Duration::from_secs(30)
                && let Some(value) = health::read_health(path).temperature_c
            {
                temperature.push((bytes, value));
                next_temperature = Instant::now();
            }
            last = now;
            last_bytes = bytes;
        }
        if finished {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    while let Ok(error) = error_rx.try_recv() {
        errors_logged += 1;
        if first_error.is_none() {
            first_error = Some(error.offset);
        }
        observer(SuiteEvent::Error {
            index,
            phase,
            offset: error.offset,
            kind: error.kind,
        });
    }

    let mut counters = Counters::default();
    let mut write_errors = 0u64;
    let mut read_errors = 0u64;
    let mut verify_errors = 0u64;
    let mut stopped_at = None;
    for handle in handles {
        let outcome = handle
            .join()
            .map_err(|_| anyhow::anyhow!("speed suite worker panicked"))??;
        counters.merge(&outcome.counters);
        write_errors += outcome.write_errors;
        read_errors += outcome.read_errors;
        verify_errors += outcome.verify_errors;
    }
    if strict && errors_logged > 0 {
        stopped_at = first_error;
    }
    let elapsed = start.elapsed();
    let metrics = counters.snapshot(elapsed);
    Ok(PhaseOutcome {
        metrics,
        curve,
        temperature,
        write_errors,
        read_errors,
        verify_errors,
        stopped: strict && errors_logged > 0,
        stopped_at,
    })
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    path: &Path,
    block_size: u64,
    is_read: bool,
    lane: usize,
    workers: usize,
    order: &[u32],
    done: &AtomicU64,
    stop: &AtomicBool,
    cancel: &AtomicBool,
    strict: bool,
    error_tx: &mpsc::Sender<WorkerError>,
    seed: u64,
) -> Result<WorkerOutcome> {
    let mut file = open_bench_file(path, true)
        .with_context(|| format!("failed to open benchmark file {}", path.display()))?;
    let mut actual = AlignedBlock::new(block_size as usize);
    let mut expected = AlignedBlock::new(block_size as usize);
    let mut counters = Counters::default();
    let mut write_errors = 0u64;
    let mut read_errors = 0u64;
    let mut verify_errors = 0u64;

    for position in (lane..order.len()).step_by(workers) {
        if cancel.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
            break;
        }
        let block = order[position] as u64;
        let offset = block * block_size;
        fill_pattern(expected.as_mut_slice(), block, seed);
        if is_read {
            let started = Instant::now();
            let result = file.read_at(offset, &mut actual);
            let latency = started.elapsed().as_nanos() as u64;
            match result {
                Ok(bytes) => {
                    counters.record(true, bytes as u64, latency);
                    if actual.as_slice() != expected.as_slice() {
                        verify_errors += 1;
                        let _ = error_tx.send(WorkerError {
                            offset,
                            kind: "verify error",
                        });
                        if strict {
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
                Err(_) => {
                    read_errors += 1;
                    let _ = error_tx.send(WorkerError {
                        offset,
                        kind: "read error",
                    });
                    if strict {
                        stop.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        } else {
            let started = Instant::now();
            let result = file.write_at(offset, &expected);
            let latency = started.elapsed().as_nanos() as u64;
            match result {
                Ok(bytes) => {
                    counters.record(false, bytes as u64, latency);
                }
                Err(_) => {
                    write_errors += 1;
                    let _ = error_tx.send(WorkerError {
                        offset,
                        kind: "write error",
                    });
                    if strict {
                        stop.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
        done.fetch_add(1, Ordering::Relaxed);
    }

    Ok(WorkerOutcome {
        counters,
        write_errors,
        read_errors,
        verify_errors,
    })
}

fn build_order(random: bool, blocks: u64, seed: u64, phase: SuitePhase) -> Vec<u32> {
    let mut order: Vec<u32> = (0..blocks as u32).collect();
    if random {
        let mut state = seed
            ^ (phase as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ blocks.wrapping_mul(0xA24B_AED4_963E_E407);
        for index in (1..order.len()).rev() {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let value = state.wrapping_mul(0x2545_F491_4F6C_DD1D) as usize;
            order.swap(index, value % (index + 1));
        }
    }
    order
}

fn fill_pattern(buffer: &mut [u8], block: u64, seed: u64) {
    let mut state = seed ^ block.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut chunks = buffer.chunks_exact_mut(8);
    for chunk in &mut chunks {
        chunk.copy_from_slice(&next().to_le_bytes());
    }
    let remainder = chunks.into_remainder();
    if !remainder.is_empty() {
        let value = next().to_le_bytes();
        remainder.copy_from_slice(&value[..remainder.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_result_failed_flags_any_error() {
        let mut item = PresetResult::default();
        assert!(!item.failed());
        item.verify_errors = 1;
        assert!(item.failed());
    }

    #[test]
    fn suite_result_can_hold_curve_history() {
        let mut result = SuiteResult::default();
        result.presets.push(PresetResult {
            write_curve: vec![(0, 10.0), (1024, 20.0)],
            ..PresetResult::default()
        });
        assert_eq!(result.presets[0].write_curve.len(), 2);
    }
    use std::io::{Seek, SeekFrom, Write};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("urwtest-rs-suite-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn preset(block_size: u64, random: bool) -> SpeedPreset {
        SpeedPreset {
            id: if random { "RND4K" } else { "SEQ4K" },
            name: if random { "RND4K" } else { "SEQ4K" },
            block_size,
            iodepth: 4,
            numjobs: 1,
            random,
        }
    }

    fn options(file_size: u64) -> SuiteOptions {
        SuiteOptions {
            file_size,
            ignore_sequential_errors: false,
            seed: 7,
        }
    }

    #[test]
    fn sequential_suite_writes_then_reads_and_verifies() {
        let dir = temp_dir("seq");
        let result = run_suite(
            &dir,
            &[preset(4096, false)],
            options(64 * 1024),
            Arc::new(AtomicBool::new(false)),
            &mut |_| {},
        )
        .expect("suite");
        assert!(!result.stopped);
        assert_eq!(result.presets.len(), 1);
        let item = &result.presets[0];
        assert_eq!(item.write.total_bytes, 64 * 1024);
        assert_eq!(item.read.total_bytes, 64 * 1024);
        assert_eq!(item.verify_errors, 0);
        assert_eq!(item.read_errors, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_suite_writes_all_blocks_before_reading() {
        let dir = temp_dir("rnd");
        let result = run_suite(
            &dir,
            &[preset(4096, true)],
            options(64 * 1024),
            Arc::new(AtomicBool::new(false)),
            &mut |_| {},
        )
        .expect("suite");
        assert!(!result.stopped);
        let item = &result.presets[0];
        assert_eq!(item.write.total_bytes, 64 * 1024);
        assert_eq!(item.read.total_bytes, 64 * 1024);
        assert_eq!(item.verify_errors, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn corrupt_first_block(path: &std::path::Path) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open");
        file.seek(SeekFrom::Start(0)).expect("seek");
        file.write_all(&[0u8; 4096]).expect("corrupt");
    }

    #[test]
    fn sequential_verify_error_stops_the_suite() {
        let dir = temp_dir("seq-corrupt");
        let path = dir.join("BENCH0001.DAT");
        let mut corrupted = false;
        let result = run_suite(
            &dir,
            &[preset(4096, false)],
            options(64 * 1024),
            Arc::new(AtomicBool::new(false)),
            &mut |event| {
                if let SuiteEvent::PhaseStarted {
                    phase: SuitePhase::Verify,
                    ..
                } = event
                    && !corrupted
                {
                    corrupted = true;
                    corrupt_first_block(&path);
                }
            },
        )
        .expect("suite");
        assert!(result.stopped);
        assert_eq!(result.stopped_at, Some(0));
        assert_eq!(result.presets[0].verify_errors, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_verify_error_is_logged_but_does_not_stop() {
        let dir = temp_dir("rnd-corrupt");
        let path = dir.join("BENCH0001.DAT");
        let mut corrupted = false;
        let result = run_suite(
            &dir,
            &[preset(4096, true)],
            options(64 * 1024),
            Arc::new(AtomicBool::new(false)),
            &mut |event| {
                if let SuiteEvent::PhaseStarted {
                    phase: SuitePhase::Verify,
                    ..
                } = event
                    && !corrupted
                {
                    corrupted = true;
                    corrupt_first_block(&path);
                }
            },
        )
        .expect("suite");
        assert!(!result.stopped);
        assert_eq!(result.presets[0].verify_errors, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
