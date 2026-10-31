//! Deterministic synthetic traffic generator (`--demo`): lets you explore the
//! whole UI (loss spikes, jitter, color thresholds) without privileges.

use std::time::{Duration, Instant};

use crate::ring::Sample;

pub struct DemoGen {
    state: u64,
    seq: u64,
    pub base_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
}

impl DemoGen {
    pub fn new(seed: u64) -> Self {
        Self { state: seed, seq: 0, base_ms: 12.0, jitter_ms: 6.0, loss_pct: 4.0 }
    }

    /// SplitMix64 — no rand dependency, reproducible from the seed.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn tick(&mut self) -> (Sample, Duration) {
        self.seq += 1;
        let lost = self.unit() * 100.0 < self.loss_pct;
        let mut ms = self.base_ms + (self.unit() - 0.5) * self.jitter_ms;
        if self.unit() > 0.94 {
            ms += 50.0 + self.unit() * 350.0; // congestion spikes
        }
        ms = ms.max(0.3);
        let ttl = Some(64 - (self.next_u64() % 4) as u8);
        let interval = Duration::from_millis(50 + (self.unit() * 120.0) as u64);
        (
            Sample {
                seq: self.seq,
                ms: if lost { 0.0 } else { ms },
                ttl,
                lost,
                src: None,
                at: Instant::now(),
            },
            interval,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_stable() {
        let mut a = DemoGen::new(9);
        let mut b = DemoGen::new(9);
        for _ in 0..500 {
            let (sa, _) = a.tick();
            let (sb, _) = b.tick();
            assert_eq!(sa.ms.to_bits(), sb.ms.to_bits());
            assert_eq!(sa.lost, sb.lost);
            assert!(!sa.lost && (sa.ms >= 0.3) || sa.lost);
        }
    }

    #[test]
    fn different_seeds_differ() {
        let (s1, _) = DemoGen::new(1).tick();
        let (s2, _) = DemoGen::new(2).tick();
        assert_ne!(s1.ms.to_bits(), s2.ms.to_bits());
    }
}
