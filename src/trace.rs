//! Per-block timing trace recorded by the engine.
//!
//! The trace is the raw material for speed charts and slowdown analysis. One
//! point is recorded when a block finishes (and at the start of each phase), so
//! any later sampling interval can be derived without re-running the test.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Write,
    Verify,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Write => "write",
            Phase::Verify => "verify",
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct TracePoint {
    pub pass: u32,
    pub phase: Phase,
    /// Bytes completed within this phase.
    pub position: u64,
    /// Milliseconds since the start of this phase.
    pub phase_ms: u64,
    /// Milliseconds since the start of the whole run.
    pub run_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RunTrace {
    pub points: Vec<TracePoint>,
}

const MAX_TRACE_POINTS: usize = 1_000_000;

impl RunTrace {
    pub fn record(&mut self, pass: u32, phase: Phase, position: u64, phase_ms: u64, run_ms: u64) {
        if self.points.len() >= MAX_TRACE_POINTS {
            self.points = decimate(&self.points);
        }
        self.points.push(TracePoint {
            pass,
            phase,
            position,
            phase_ms,
            run_ms,
        });
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

fn decimate(points: &[TracePoint]) -> Vec<TracePoint> {
    let mut output = Vec::with_capacity(points.len() / 2 + 2);
    let mut start = 0usize;
    while start < points.len() {
        let mut end = start + 1;
        while end < points.len()
            && points[end].pass == points[start].pass
            && points[end].phase == points[start].phase
        {
            end += 1;
        }
        let group = &points[start..end];
        if group.len() <= 2 {
            output.extend_from_slice(group);
        } else {
            output.push(group[0]);
            for point in group.iter().skip(1).take(group.len() - 2).step_by(2) {
                output.push(*point);
            }
            output.push(group[group.len() - 1]);
        }
        start = end;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_preserves_phase_endpoints() {
        let mut trace = RunTrace::default();
        for index in 0..8 {
            trace.record(1, Phase::Write, index * 1024, index, index);
        }
        let decimated = decimate(&trace.points);
        assert_eq!(decimated.first().unwrap().position, 0);
        assert_eq!(decimated.last().unwrap().position, 7 * 1024);
        assert!(decimated.len() < trace.points.len());
    }
}
