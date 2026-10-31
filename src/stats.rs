//! Streaming statistics: counts, min/avg/max/mdev, jitter, sliding-window
//! percentiles.

use std::collections::BTreeMap;
use std::cmp::Ordering;

/// Window of recent latencies kept for percentile queries.
pub const WINDOW_MAX: usize = 4000;

/// f64 wrapper with a total order so it can key a BTreeMap.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Latency(f64);

impl Latency {
    fn value(self) -> f64 {
        self.0
    }
}

impl Ord for Latency {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

impl std::cmp::Eq for Latency {}

impl PartialOrd for Latency {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::hash::Hash for Latency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state)
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub transmitted: u64,
    pub received: u64,
    pub lost: u64,
    pub cur_min: f64,
    pub cur_max: f64,
    sum: f64,
    sum_sq: f64,
    last_ms: Option<f64>,
    jitter_sum: f64,
    jitter_n: u64,
    win: BTreeMap<Latency, usize>,
    win_n: usize,
}

impl Stats {
    pub fn record_reply(&mut self, ms: f64) {
        self.received += 1;
        if self.received == 1 || ms < self.cur_min {
            self.cur_min = ms;
        }
        if ms > self.cur_max {
            self.cur_max = ms;
        }
        self.sum += ms;
        self.sum_sq += ms * ms;
        if let Some(prev) = self.last_ms {
            self.jitter_sum += (ms - prev).abs();
            self.jitter_n += 1;
        }
        self.last_ms = Some(ms);
        *self.win.entry(Latency(ms)).or_insert(0) += 1;
        self.win_n += 1;
        while self.win_n > WINDOW_MAX {
            match self.win.iter().next().map(|(k, _)| *k) {
                Some(k) => {
                    if let Some(c) = self.win.get_mut(&k) {
                        *c -= 1;
                        if *c == 0 {
                            self.win.remove(&k);
                        }
                    }
                    self.win_n -= 1;
                }
                None => break,
            }
        }
    }

    pub fn record_lost(&mut self) {
        self.lost += 1;
        // A break in the sequence invalidates the previous inter-packet delta.
        self.last_ms = None;
    }

    pub fn loss_pct(&self) -> f64 {
        let done = self.received + self.lost;
        let total = self.transmitted.max(done);
        if total == 0 {
            0.0
        } else {
            self.lost as f64 * 100.0 / total as f64
        }
    }

    pub fn mean(&self) -> f64 {
        if self.received == 0 {
            0.0
        } else {
            self.sum / self.received as f64
        }
    }

    /// Sample standard deviation (what gping labels `stddev`).
    pub fn stddev(&self) -> f64 {
        if self.received < 2 {
            return 0.0;
        }
        let n = self.received as f64;
        let var = (self.sum_sq - self.sum * self.sum / n) / (n - 1.0);
        var.max(0.0).sqrt()
    }

    /// iputils-style mdev: mean absolute deviation from the mean, over the
    /// recent window (exact, no numerical cancellation).
    pub fn mdev(&self) -> f64 {
        if self.received < 2 || self.win_n == 0 {
            return 0.0;
        }
        let mean = self.mean();
        let acc: f64 = self
            .win
            .iter()
            .map(|(k, c)| (k.value() - mean).abs() * *c as f64)
            .sum();
        acc / self.win_n as f64
    }

    /// RFC 3550 "interarrival jitter": mean of |rtt[i] − rtt[i−1]|.
    pub fn jitter_ms(&self) -> f64 {
        if self.jitter_n == 0 {
            0.0
        } else {
            self.jitter_sum / self.jitter_n as f64
        }
    }

    /// Nearest-rank percentile (q in 0..=1) over the recent window.
    pub fn percentile(&self, q: f64) -> f64 {
        if self.win_n == 0 {
            return 0.0;
        }
        let rank = ((q * self.win_n as f64).ceil() as usize).clamp(1, self.win_n);
        let mut seen = 0usize;
        for (k, c) in self.win.iter() {
            seen += c;
            if seen >= rank {
                return k.value();
            }
        }
        self.cur_max
    }

    pub fn sorted_window(&self) -> Vec<f64> {
        let mut v = Vec::with_capacity(self.win_n);
        for (k, c) in self.win.iter() {
            for _ in 0..*c {
                v.push(k.value());
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics_and_percentiles() {
        let mut s = Stats::default();
        for i in 0..100 {
            s.transmitted += 1;
            s.record_reply(i as f64);
        }
        assert_eq!(s.received, 100);
        assert!((s.mean() - 49.5).abs() < 1e-9);
        assert!((s.percentile(0.5) - 49.0).abs() < 1.0);
        assert!((s.percentile(0.99) - 98.0).abs() < 1.0);
        assert_eq!(s.cur_min, 0.0);
        assert_eq!(s.cur_max, 99.0);
        // sample stddev of 0..99 ≈ 29.01
        assert!((s.stddev() - 29.011).abs() < 0.05);
    }

    #[test]
    fn loss_and_jitter_and_mdev() {
        let mut s = Stats::default();
        s.transmitted = 4;
        s.record_reply(10.0);
        s.record_lost();
        s.record_reply(20.0); // no jitter across the lost gap
        s.record_reply(30.0);
        assert_eq!(s.lost, 1);
        assert!((s.loss_pct() - 25.0).abs() < 1e-9);
        assert!((s.jitter_ms() - 10.0).abs() < 1e-9); // only |30−20| counted
        assert!((s.mdev() - (20.0 / 3.0)).abs() < 1e-9);
        assert!((s.mean() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn window_eviction() {
        let mut s = Stats::default();
        for i in 0..(WINDOW_MAX + 500) {
            s.record_reply(i as f64);
        }
        assert_eq!(s.win_n, WINDOW_MAX);
        let w = s.sorted_window();
        assert_eq!(w[0], 500.0);
        assert_eq!(w[w.len() - 1], (WINDOW_MAX + 499) as f64);
        assert!(s.percentile(0.01) >= 500.0);
    }

    #[test]
    fn empty_defaults() {
        let s = Stats::default();
        assert_eq!(s.loss_pct(), 0.0);
        assert_eq!(s.mean(), 0.0);
        assert_eq!(s.stddev(), 0.0);
        assert_eq!(s.mdev(), 0.0);
        assert_eq!(s.percentile(0.5), 0.0);
        assert_eq!(s.jitter_ms(), 0.0);
        assert!(s.sorted_window().is_empty());
    }
}
