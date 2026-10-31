//! Rendering helpers: formatting, color levels, sparklines, heatmaps.
//! Pure functions (no ratatui/egui types) so they can be unit-tested.

use std::time::Duration;

use crate::ring::Sample;

/// Human-friendly latency formatting (adaptive precision).
pub fn fmt_ms(ms: f64) -> String {
    if ms <= 0.0 {
        "0".into()
    } else if ms < 10.0 {
        format!("{ms:.2}")
    } else if ms < 100.0 {
        format!("{ms:.1}")
    } else {
        format!("{ms:.0}")
    }
}

/// Traffic-light level index (0=green, 1=yellow, 2=orange, 3=red) for a latency.
/// Bands are defined by the ascending thresholds: green ≤ A, yellow ≤ B,
/// orange ≤ C, red ≤ D, and anything above D still maps to red (index 3).
pub fn level_for(ms: f64, t: &[f64; 4]) -> usize {
    let mut lvl = 0;
    for (i, &th) in t.iter().enumerate() {
        if ms > th {
            lvl = i + 1;
        }
    }
    lvl.min(3)
}

const SPARK_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Map values to 8-level block characters (lost samples → `lost_char`).
/// Used by the ASCII summary printed after the TUI session.
pub fn sparkline_cells(values: &[f64], width: usize, lost_char: char) -> String {
    if values.is_empty() {
        return "·".repeat(width.min(1024));
    }
    let hi = values.iter().cloned().fold(f64::MIN, f64::max).max(1.0);
    let n = width.min(values.len().max(1));
    let cells: String = (0..n)
        .map(|i| {
            let v = values[i];
            if v < 0.0 {
                lost_char
            } else {
                let lvl = ((v / hi) * 7.0).round().clamp(0.0, 7.0) as usize;
                SPARK_BLOCKS[lvl]
            }
        })
        .collect();
    if n < width {
        format!("{}{}", " ".repeat(width - n), cells)
    } else {
        cells
    }
}

/// Round a latency up to a “nice” axis maximum (1/2/5 × 10ⁿ).
pub fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let e = 10f64.powf(v.log10().floor());
    let m = (v / e).ceil().clamp(1.0, 10.0);
    (m * e).max(1.0)
}

/// 8-cell horizontal bar (█ filled / ░ empty) scaled to `hi`.
pub fn bar8(v: f64, hi: f64) -> String {
    let frac = (v / hi.max(1.0)).clamp(0.0, 1.0);
    let n = (frac * 8.0).round() as usize;
    let mut s = String::with_capacity(8);
    for i in 0..8 {
        s.push(if i < n { '█' } else { '░' });
    }
    s
}

/// Timeline/heatmap grid of packet cells, oldest at top-left, newest at
/// bottom-right. Cell values: 0 = empty, 1..=4 = latency level (+1),
/// 5 = packet lost. Also returns the total time span in seconds covered
/// by `rows × cols` cells.
pub fn heatmap(samples: &[&Sample], rows: usize, cols: usize, interval: Duration, t: &[f64; 4]) -> (Vec<Vec<u8>>, f64) {
    let mut g = vec![vec![0u8; cols]; rows];
    if rows == 0 || cols == 0 || samples.is_empty() {
        return (g, 0.0);
    }
    let bucket = interval.as_secs_f64().max(1e-6);
    let total_cells = rows * cols;
    for (idx, s) in samples.iter().rev().enumerate().take(total_cells) {
        let col = cols - 1 - (idx % cols);
        let row = rows - 1 - (idx / cols);
        let p: u8 = if s.lost { 5 } else { (level_for(s.ms, t) + 1) as u8 };
        g[row][col] = g[row][col].max(p);
    }
    (g, total_cells as f64 * bucket)
}

/// Horizontal bins with log-ish spacing (fine near 0, coarse at the top).
/// Returns `None` when there is nothing to plot.
pub fn histogram(latencies: &[f64], bins: usize) -> Option<(Vec<f64>, Vec<usize>)> {
    if latencies.is_empty() {
        return None;
    }
    let max = latencies.iter().cloned().fold(f64::MIN, f64::max);
    let mut edges = vec![0.0f64];
    for i in 1..=bins {
        let f = i as f64 / bins as f64;
        edges.push(max * (100.0f64.powf(f) - 1.0) / 99.0);
    }
    let mut counts = vec![0usize; bins];
    for &v in latencies {
        let mut i = match edges.binary_search_by(|e| e.partial_cmp(&v).unwrap_or(std::cmp::Ordering::Less)) {
            Ok(k) => k.min(bins - 1),
            Err(k) => k.saturating_sub(1).min(bins - 1),
        };
        if v >= edges[bins] {
            i = bins - 1;
        }
        counts[i] += 1;
    }
    Some((edges, counts))
}

#[cfg(test)]
pub fn mdev_ad(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    values.iter().map(|v| (v - mean).abs()).sum::<f64>() / n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn s(ms: f64, lost: bool) -> Sample {
        Sample { seq: 1, ms, ttl: None, lost, src: None, at: Instant::now() }
    }

    #[test]
    fn thresholds_levels() {
        let t = [25.0, 75.0, 150.0, 300.0];
        assert_eq!(level_for(10.0, &t), 0);
        assert_eq!(level_for(30.0, &t), 1);
        assert_eq!(level_for(80.0, &t), 2);
        assert_eq!(level_for(200.0, &t), 3);
        assert_eq!(level_for(500.0, &t), 3); // saturates at the 4th color
    }

    #[test]
    fn spark_extremes() {
        assert_eq!(sparkline_cells(&[], 4, '×'), "····");
        assert_eq!(sparkline_cells(&[0.0], 1, '×').chars().next().unwrap(), '▁');
        assert_eq!(sparkline_cells(&[999.0], 1, '×').chars().next().unwrap(), '█');
        assert_eq!(sparkline_cells(&[-1.0], 1, '×'), "×");
        assert_eq!(sparkline_cells(&[1.0, 2.0], 4, '×').chars().count(), 4);
    }

    #[test]
    fn nice_ceil_axis_maxima() {
        assert_eq!(nice_ceil(0.0), 1.0);
        assert_eq!(nice_ceil(3.2), 4.0);
        assert_eq!(nice_ceil(181.0), 200.0);
        assert_eq!(nice_ceil(1000.0), 1000.0);
        assert_eq!(nice_ceil(1001.0), 2000.0);
    }

    #[test]
    fn bar_chars() {
        assert_eq!(bar8(50.0, 100.0), "████░░░░");
        assert_eq!(bar8(0.0, 100.0), "░░░░░░░░");
        assert_eq!(bar8(150.0, 100.0), "████████");
    }

    #[test]
    fn heatmap_orders_newest_bottom_right() {
        let t = [25.0, 75.0, 150.0, 300.0];
        let a = s(1.0, false); // plus ancien
        let b = s(200.0, false);
        let c = s(0.0, true); // plus récent, perdu
        let grid = heatmap(&[&a, &b, &c], 2, 2, Duration::from_millis(1000), &t).0;
        assert_eq!(grid, vec![vec![0, 1], vec![4, 5]]);
    }

    #[test]
    fn heatmap_empty_inputs() {
        let t = [25.0, 75.0, 150.0, 300.0];
        let (g, span) = heatmap(&[], 3, 4, Duration::from_millis(1000), &t);
        assert_eq!(g.len(), 3);
        assert_eq!(span, 0.0);
    }

    #[test]
    fn fmt_units() {
        assert_eq!(fmt_ms(3.456), "3.46");
        assert_eq!(fmt_ms(45.67), "45.7");
        assert_eq!(fmt_ms(123.4), "123");
        assert_eq!(fmt_ms(0.0), "0");
    }

    #[test]
    fn histogram_covers_all() {
        let data = [1.0, 2.0, 3.0, 40.0, 100.0];
        let (edges, counts) = histogram(&data, 8).unwrap();
        assert_eq!(counts.iter().sum::<usize>(), data.len());
        assert_eq!(edges.len(), 9);
        assert_eq!(edges[0], 0.0);
        assert!((edges[8] - 100.0).abs() < 1e-9);
        assert!(histogram(&[], 8).is_none());
    }

    #[test]
    fn mdev_reference() {
        let v = [4.0, 8.0, 12.0];
        assert!((mdev_ad(&v) - 2.666_666).abs() < 1e-4);
    }
}
