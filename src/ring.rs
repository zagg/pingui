//! Fixed-capacity ring of samples (bounded memory, `--count` is a soft cap).

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Sample {
    pub seq: u64,
    /// Round-trip time in milliseconds (0.0 when `lost`).
    pub ms: f64,
    pub ttl: Option<u8>,
    pub lost: bool,
    pub src: Option<std::net::IpAddr>,
    pub at: std::time::Instant,
}

pub struct Ring {
    buf: VecDeque<Sample>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap), cap }
    }

    pub fn push(&mut self, s: Sample) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(s);
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[allow(dead_code)] // paired with len() (clippy) and handy for tests
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Oldest → newest. (VecDeque iterator: DoubleEnded + ExactSize on purpose.)
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, Sample> {
        self.buf.iter()
    }

    pub fn back(&self) -> Option<&Sample> {
        self.buf.back()
    }

    /// Latency values ready to feed `viz::sparkline_cells` (lost → −1).
    pub fn spark_values(&self) -> Vec<f64> {
        self.buf.iter().map(|s| if s.lost { -1.0 } else { s.ms }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sample(seq: u64) -> Sample {
        Sample { seq, ms: seq as f64, ttl: None, lost: false, src: None, at: Instant::now() }
    }

    #[test]
    fn overwrites_oldest_when_full() {
        let mut r = Ring::new(3);
        for i in 1..=5 {
            r.push(sample(i));
        }
        let seqs: Vec<u64> = r.iter().map(|s| s.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
        assert_eq!(r.len(), 3);
        assert_eq!(r.back().unwrap().seq, 5);
    }

    #[test]
    fn clear_and_spark_values() {
        let mut r = Ring::new(4);
        r.push(sample(1));
        let mut lost = sample(2);
        lost.lost = true;
        r.push(lost);
        let v = r.spark_values();
        assert_eq!(v, vec![1.0, -1.0]);
        r.clear();
        assert!(r.is_empty());
    }
}
