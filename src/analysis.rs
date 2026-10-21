//! Speed analysis: resampling and slowdown detection.
//!
//! The engine records one timing point per finished block. This module turns
//! that raw trace into a fixed-interval speed series and detects slow episodes.
//! All thresholds are analysis parameters, so the result can be recomputed
//! after a run while the user inspects the chart.

use crate::trace::{Phase, RunTrace, TracePoint};

const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdMode {
    Percent,
    Absolute,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct AnalysisParams {
    pub mode: ThresholdMode,
    /// Percent of the rolling baseline (used by ThresholdMode::Percent).
    pub percent: f64,
    /// Absolute threshold in MiB/s (used by ThresholdMode::Absolute).
    pub absolute_mibps: f64,
    /// Minimum duration for a slow episode; filters momentary dips.
    pub min_duration_ms: u64,
    /// Speed must recover to this percent of the baseline to close an episode.
    pub recovery_percent: f64,
    /// Episodes shorter than this are classified as spikes.
    pub recovery_window_ms: u64,
    /// Window used for the rolling baseline.
    pub baseline_window_ms: u64,
    /// Output sampling interval.
    pub sample_interval_ms: u64,
    /// Window used to compute the speed at each sample.
    pub window_ms: u64,
    /// Episodes closer than this are merged.
    pub merge_gap_ms: u64,
}

impl Default for AnalysisParams {
    fn default() -> Self {
        Self {
            mode: ThresholdMode::Percent,
            percent: 80.0,
            absolute_mibps: 100.0,
            min_duration_ms: 150,
            recovery_percent: 90.0,
            recovery_window_ms: 3000,
            baseline_window_ms: 60_000,
            sample_interval_ms: 100,
            window_ms: 100,
            merge_gap_ms: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlowKind {
    Spike,
    Sustained,
}

impl SlowKind {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            SlowKind::Spike => "spike",
            SlowKind::Sustained => "sustained",
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SpeedSample {
    pub pass: u32,
    pub phase: Phase,
    pub run_ms: u64,
    pub phase_ms: u64,
    pub position: u64,
    pub speed_mibps: f64,
    pub average_mibps: f64,
    pub baseline_mibps: f64,
    pub threshold_mibps: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SlowEvent {
    pub pass: u32,
    pub phase: Phase,
    pub kind: SlowKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub start_position: u64,
    pub end_position: u64,
    pub duration_ms: u64,
    pub min_mibps: f64,
    pub baseline_mibps: f64,
    pub depth_percent: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub samples: Vec<SpeedSample>,
    pub events: Vec<SlowEvent>,
    pub peak_mibps: f64,
    pub mean_mibps: f64,
    pub min_mibps: f64,
}

#[derive(Clone, Copy)]
struct SegmentPoint {
    phase_ms: u64,
    position: u64,
    run_ms: u64,
}

struct Segment {
    pass: u32,
    phase: Phase,
    points: Vec<SegmentPoint>,
}

pub fn analyze(trace: &RunTrace, params: &AnalysisParams) -> Analysis {
    let mut analysis = Analysis::default();
    for segment in split_segments(&trace.points) {
        analyze_segment(&segment, params, &mut analysis);
    }
    if !analysis.samples.is_empty() {
        let count = analysis.samples.len() as f64;
        let sum: f64 = analysis
            .samples
            .iter()
            .map(|sample| sample.speed_mibps)
            .sum();
        analysis.mean_mibps = sum / count;
        analysis.peak_mibps = analysis
            .samples
            .iter()
            .map(|sample| sample.speed_mibps)
            .fold(0.0, f64::max);
        analysis.min_mibps = analysis
            .samples
            .iter()
            .map(|sample| sample.speed_mibps)
            .fold(f64::INFINITY, f64::min);
    }
    analysis
}

fn split_segments(points: &[TracePoint]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut current_key: Option<(u32, Phase)> = None;
    for point in points {
        let key = (point.pass, point.phase);
        if current_key != Some(key) {
            segments.push(Segment {
                pass: point.pass,
                phase: point.phase,
                points: Vec::new(),
            });
            current_key = Some(key);
        }
        if let Some(segment) = segments.last_mut() {
            segment.points.push(SegmentPoint {
                phase_ms: point.phase_ms,
                position: point.position,
                run_ms: point.run_ms,
            });
        }
    }
    segments.retain(|segment| segment.points.len() >= 2);
    segments
}

fn analyze_segment(segment: &Segment, params: &AnalysisParams, analysis: &mut Analysis) {
    let points = &segment.points;
    let Some(last) = points.last() else {
        return;
    };
    for pair in points.windows(2) {
        if pair[1].phase_ms < pair[0].phase_ms {
            return;
        }
    }
    let duration_ms = last.phase_ms.max(1);
    let interval = params.sample_interval_ms.max(10);
    if duration_ms < interval {
        return;
    }

    let mut samples: Vec<SpeedSample> = Vec::new();
    let mut time = 0u64;
    while time <= duration_ms {
        let position = position_at(points, time);
        let window_start = time.saturating_sub(params.window_ms);
        let window_position = position_at(points, window_start);
        let window_seconds = (time - window_start) as f64 / 1000.0;
        let speed = if window_seconds > 0.001 {
            (position.saturating_sub(window_position)) as f64 / window_seconds / MIB
        } else {
            0.0
        };
        let average = if time > 0 {
            position as f64 / (time as f64 / 1000.0) / MIB
        } else {
            0.0
        };
        let run_ms = last.run_ms.saturating_sub(last.phase_ms) + time;
        samples.push(SpeedSample {
            pass: segment.pass,
            phase: segment.phase,
            run_ms,
            phase_ms: time,
            position,
            speed_mibps: speed,
            average_mibps: average,
            baseline_mibps: 0.0,
            threshold_mibps: 0.0,
        });
        time += interval;
    }

    if samples.len() < 3 {
        return;
    }

    // Causal baseline: a rolling median of recent non-slow samples. Slow
    // samples never feed the baseline, so a sustained slowdown does not drag
    // the reference down with it.
    let (slow, baselines) = causal_baselines(&samples, params);
    for (sample, (baseline, threshold)) in samples.iter_mut().zip(baselines) {
        sample.baseline_mibps = baseline;
        sample.threshold_mibps = threshold;
    }

    detect_events(&samples, &slow, params, analysis);
    analysis.samples.extend(samples);
}

fn causal_baselines(
    samples: &[SpeedSample],
    params: &AnalysisParams,
) -> (Vec<bool>, Vec<(f64, f64)>) {
    let interval = samples
        .get(1)
        .map(|sample| sample.phase_ms.saturating_sub(samples[0].phase_ms))
        .unwrap_or(params.sample_interval_ms)
        .max(1);
    let capacity = (params.baseline_window_ms / interval).max(1) as usize;
    let warmup = samples.len().clamp(1, 5);

    let mut history: std::collections::VecDeque<f64> = samples[..warmup]
        .iter()
        .map(|sample| sample.speed_mibps)
        .collect();
    let mut reference = {
        let mut values: Vec<f64> = history.iter().copied().collect();
        median(&mut values)
    };

    let mut slow = Vec::with_capacity(samples.len());
    let mut values = Vec::with_capacity(samples.len());
    for sample in samples {
        if sample.phase_ms < params.window_ms {
            slow.push(false);
            values.push((reference, 0.0));
            continue;
        }
        let threshold = threshold_for(reference, params);
        let is_slow = threshold > 0.0 && sample.speed_mibps < threshold;
        slow.push(is_slow);
        values.push((reference, threshold));
        if !is_slow && sample.speed_mibps > 0.0 {
            history.push_back(sample.speed_mibps);
            while history.len() > capacity {
                history.pop_front();
            }
            let mut sorted: Vec<f64> = history.iter().copied().collect();
            reference = median(&mut sorted);
        }
    }
    (slow, values)
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mid = values.len() / 2;
    values.select_nth_unstable_by(mid, f64::total_cmp);
    values[mid]
}

fn threshold_for(baseline: f64, params: &AnalysisParams) -> f64 {
    match params.mode {
        ThresholdMode::Percent => baseline * params.percent / 100.0,
        ThresholdMode::Absolute => params.absolute_mibps,
    }
}

#[allow(clippy::needless_range_loop)]
fn detect_events(
    samples: &[SpeedSample],
    slow: &[bool],
    params: &AnalysisParams,
    analysis: &mut Analysis,
) {
    let interval = samples
        .get(1)
        .map(|sample| sample.phase_ms.saturating_sub(samples[0].phase_ms))
        .unwrap_or(params.sample_interval_ms)
        .max(1);
    let min_samples = params.min_duration_ms.div_ceil(interval).max(1) as usize;

    let mut run_start: Option<usize> = None;
    let mut events: Vec<SlowEvent> = Vec::new();

    for index in 0..=samples.len() {
        let is_slow = index < samples.len() && slow[index];
        if is_slow {
            if run_start.is_none() {
                run_start = Some(index);
            }
            continue;
        }

        if let Some(start) = run_start.take() {
            if index - start < min_samples {
                continue;
            }
            let end = index.saturating_sub(1);
            let first = &samples[start];
            let last = &samples[end];
            let baseline = first.baseline_mibps.max(last.baseline_mibps);
            let min_speed = samples[start..=end]
                .iter()
                .map(|sample| sample.speed_mibps)
                .fold(f64::INFINITY, f64::min);
            let duration_ms = last.phase_ms.saturating_sub(first.phase_ms) + interval;
            let kind = if duration_ms <= params.recovery_window_ms {
                SlowKind::Spike
            } else {
                SlowKind::Sustained
            };
            events.push(SlowEvent {
                pass: first.pass,
                phase: first.phase,
                kind,
                start_ms: first.run_ms,
                end_ms: last.run_ms,
                start_position: first.position,
                end_position: last.position,
                duration_ms,
                min_mibps: min_speed,
                baseline_mibps: baseline,
                depth_percent: if baseline > 0.0 {
                    (1.0 - min_speed / baseline) * 100.0
                } else {
                    0.0
                },
            });
        }
    }

    events.sort_by_key(|event| event.start_ms);
    let mut merged: Vec<SlowEvent> = Vec::new();
    for event in events {
        if let Some(previous) = merged.last_mut()
            && event.start_ms.saturating_sub(previous.end_ms) <= params.merge_gap_ms
        {
            previous.end_ms = event.end_ms;
            previous.end_position = event.end_position;
            previous.duration_ms = previous.end_ms.saturating_sub(previous.start_ms);
            previous.min_mibps = previous.min_mibps.min(event.min_mibps);
            previous.depth_percent = previous.depth_percent.max(event.depth_percent);
            continue;
        }
        merged.push(event);
    }
    for event in &mut merged {
        event.kind = if event.duration_ms <= params.recovery_window_ms {
            SlowKind::Spike
        } else {
            SlowKind::Sustained
        };
    }
    for event in &mut merged {
        event.kind = if event.duration_ms <= params.recovery_window_ms {
            SlowKind::Spike
        } else {
            SlowKind::Sustained
        };
    }
    analysis.events.extend(merged);
}

fn position_at(segment: &[SegmentPoint], time_ms: u64) -> u64 {
    if segment.is_empty() {
        return 0;
    }
    if time_ms <= segment[0].phase_ms {
        return segment[0].position;
    }
    let last = segment[segment.len() - 1];
    if time_ms >= last.phase_ms {
        return last.position;
    }
    let index = segment.partition_point(|point| point.phase_ms <= time_ms);
    let before = &segment[index - 1];
    let after = &segment[index];
    let span = after.phase_ms.saturating_sub(before.phase_ms);
    if span == 0 {
        return after.position;
    }
    let offset = time_ms - before.phase_ms;
    let delta = after.position as i128 - before.position as i128;
    let interpolated = before.position as i128 + delta * offset as i128 / span as i128;
    interpolated.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_from_blocks(runs: &[(f64, u64)]) -> RunTrace {
        let mut trace = RunTrace::default();
        let mut time_ms = 0u64;
        let mut position = 0u64;
        trace.record(1, Phase::Write, 0, 0, 0);
        for (speed, blocks) in runs {
            let block_time_ms = (1000.0 / speed).round().max(1.0) as u64;
            for _ in 0..*blocks {
                time_ms += block_time_ms;
                position += 1024 * 1024;
                trace.record(1, Phase::Write, position, time_ms, time_ms);
            }
        }
        trace
    }

    #[test]
    fn detects_short_spike() {
        let trace = trace_from_blocks(&[(100.0, 100), (10.0, 5), (100.0, 100)]);
        let analysis = analyze(&trace, &AnalysisParams::default());
        assert!(!analysis.events.is_empty());
        let event = &analysis.events[0];
        assert_eq!(event.kind, SlowKind::Spike);
        assert!(event.depth_percent > 40.0);
    }

    #[test]
    fn detects_sustained_slowdown() {
        let trace = trace_from_blocks(&[(100.0, 100), (20.0, 300)]);
        let analysis = analyze(&trace, &AnalysisParams::default());
        assert!(!analysis.events.is_empty());
        assert_eq!(analysis.events[0].kind, SlowKind::Sustained);
        assert!(analysis.events[0].duration_ms > 3000);
    }

    #[test]
    fn filters_momentary_dip() {
        let trace = trace_from_blocks(&[(100.0, 100), (10.0, 1), (100.0, 100)]);
        let analysis = analyze(&trace, &AnalysisParams::default());
        assert!(analysis.events.is_empty());
    }

    #[test]
    fn absolute_threshold_mode_works() {
        let trace = trace_from_blocks(&[(100.0, 100), (50.0, 100)]);
        let params = AnalysisParams {
            mode: ThresholdMode::Absolute,
            absolute_mibps: 80.0,
            ..AnalysisParams::default()
        };
        let analysis = analyze(&trace, &params);
        assert!(!analysis.events.is_empty());
    }
}
