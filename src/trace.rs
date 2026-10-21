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

impl RunTrace {
    pub fn record(&mut self, pass: u32, phase: Phase, position: u64, phase_ms: u64, run_ms: u64) {
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
