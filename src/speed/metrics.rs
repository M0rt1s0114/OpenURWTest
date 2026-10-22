//! Speed test counters, latency histogram, and snapshots.

use std::time::Duration;

const HISTOGRAM_BUCKETS: usize = 64;

#[derive(Debug, Clone)]
pub struct Counters {
    read_ios: u64,
    write_ios: u64,
    read_bytes: u64,
    write_bytes: u64,
    latency_sum_ns: u128,
    latency_max_ns: u64,
    histogram: [u64; HISTOGRAM_BUCKETS],
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            read_ios: 0,
            write_ios: 0,
            read_bytes: 0,
            write_bytes: 0,
            latency_sum_ns: 0,
            latency_max_ns: 0,
            histogram: [0; HISTOGRAM_BUCKETS],
        }
    }
}

impl Counters {
    pub fn merge(&mut self, other: &Counters) {
        self.read_ios += other.read_ios;
        self.write_ios += other.write_ios;
        self.read_bytes += other.read_bytes;
        self.write_bytes += other.write_bytes;
        self.latency_sum_ns += other.latency_sum_ns;
        self.latency_max_ns = self.latency_max_ns.max(other.latency_max_ns);
        for (target, source) in self.histogram.iter_mut().zip(other.histogram.iter()) {
            *target += *source;
        }
    }

    pub fn record(&mut self, is_read: bool, bytes: u64, latency_ns: u64) {
        if is_read {
            self.read_ios += 1;
            self.read_bytes += bytes;
        } else {
            self.write_ios += 1;
            self.write_bytes += bytes;
        }
        self.latency_sum_ns += latency_ns as u128;
        self.latency_max_ns = self.latency_max_ns.max(latency_ns);
        self.histogram[bucket_for(latency_ns)] += 1;
    }

    pub fn snapshot(&self, elapsed: Duration) -> MetricsSnapshot {
        let seconds = elapsed.as_secs_f64().max(0.001);
        let total_ios = self.read_ios + self.write_ios;
        let total_bytes = self.read_bytes + self.write_bytes;
        MetricsSnapshot {
            read_iops: self.read_ios as f64 / seconds,
            write_iops: self.write_ios as f64 / seconds,
            read_mibps: self.read_bytes as f64 / seconds / (1024.0 * 1024.0),
            write_mibps: self.write_bytes as f64 / seconds / (1024.0 * 1024.0),
            avg_latency_ns: if total_ios == 0 {
                0.0
            } else {
                self.latency_sum_ns as f64 / total_ios as f64
            },
            p50_latency_ns: percentile(&self.histogram, total_ios, 50),
            p95_latency_ns: percentile(&self.histogram, total_ios, 95),
            p99_latency_ns: percentile(&self.histogram, total_ios, 99),
            max_latency_ns: self.latency_max_ns,
            total_ios,
            total_bytes,
            elapsed_secs: seconds,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetricsSnapshot {
    pub read_iops: f64,
    pub write_iops: f64,
    pub read_mibps: f64,
    pub write_mibps: f64,
    pub avg_latency_ns: f64,
    pub p50_latency_ns: u64,
    pub p95_latency_ns: u64,
    pub p99_latency_ns: u64,
    pub max_latency_ns: u64,
    pub total_ios: u64,
    pub total_bytes: u64,
    pub elapsed_secs: f64,
}

fn bucket_for(latency_ns: u64) -> usize {
    let value = latency_ns.max(1);
    (64 - value.leading_zeros()) as usize - 1
}

fn percentile(histogram: &[u64; HISTOGRAM_BUCKETS], total: u64, percent: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let target = (total as u128 * percent as u128).div_ceil(100).max(1) as u64;
    let mut seen = 0;
    for (index, count) in histogram.iter().enumerate() {
        seen += *count;
        if seen >= target {
            return if index == 0 {
                1
            } else {
                (1u64 << index) | (1u64 << (index - 1))
            };
        }
    }
    1u64 << (HISTOGRAM_BUCKETS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_is_zero() {
        let snapshot = Counters::default().snapshot(Duration::from_secs(1));
        assert_eq!(snapshot.total_ios, 0);
        assert_eq!(snapshot.avg_latency_ns, 0.0);
    }

    #[test]
    fn snapshot_reports_rate_and_latency() {
        let mut counters = Counters::default();
        counters.record(true, 4096, 1_000);
        counters.record(true, 4096, 2_000);
        counters.record(false, 4096, 3_000);
        counters.record(false, 4096, 4_000);
        let snapshot = counters.snapshot(Duration::from_secs(1));
        assert_eq!(snapshot.total_ios, 4);
        assert_eq!(snapshot.total_bytes, 16_384);
        assert!((snapshot.read_iops - 2.0).abs() < f64::EPSILON);
        assert!((snapshot.avg_latency_ns - 2_500.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.max_latency_ns, 4_000);
        assert!(snapshot.p50_latency_ns <= snapshot.p95_latency_ns);
        assert!(snapshot.p95_latency_ns <= snapshot.p99_latency_ns);
    }

    #[test]
    fn latency_percentiles_use_the_bucket_floor() {
        let mut counters = Counters::default();
        for _ in 0..10 {
            counters.record(true, 4096, 1_000);
        }
        let snapshot = counters.snapshot(Duration::from_secs(1));
        assert_eq!(snapshot.p50_latency_ns, 768);
        assert_eq!(snapshot.p99_latency_ns, 768);
    }
}
