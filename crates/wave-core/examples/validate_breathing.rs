//! Validate the DSP pipeline against the real WiFi-CSI-MiningTool breathing
//! dataset (Linux 802.11n CSI Tool, 30 subcarriers × 3 antennas = 90 columns).
//!
//! Usage: cargo run --release --example validate_breathing -- <amp.csv> <fs_hz> <expected_bpm>
//!
//! Reads CSI amplitude rows, conditions each column (Hampel), runs the windowed
//! FFT respiration estimator, fuses the highest-confidence subcarriers, and
//! reports the recovered breathing rate vs the expected label.

use std::env;
use std::fs;
use wave_core::dsp::{estimate_rate_bpm, hampel, FftScratch};

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let fs: f32 = args[2].parse().unwrap();
    let expected: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(f32::NAN);

    let text = fs::read_to_string(path).expect("read csv");
    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect())
        .collect();
    let n_cols = rows[0].len();
    let n_rows = rows.len();
    println!("loaded {} rows × {} cols, Fs={} Hz", n_rows, n_cols, fs);

    // Largest power-of-two window we can fill, capped at 4096 (~164 s at 25 Hz).
    let n = (1usize << (usize::BITS - 1 - n_rows.next_power_of_two().leading_zeros())).min(4096);
    let n = if n > n_rows { n / 2 } else { n };
    let mut scratch = FftScratch::new(n);
    println!("window N={} ({:.1} s)", n, n as f32 / fs);

    let mut results: Vec<(usize, f32, f32)> = Vec::new(); // (col, bpm, confidence)
    #[allow(clippy::needless_range_loop)] // column index also selects rows[r][c]
    for c in 0..n_cols {
        let mut series: Vec<f32> = (0..n).map(|r| rows[r][c]).collect();
        hampel(&mut series, 5, 3.0);
        if let Some(est) = estimate_rate_bpm(&series, fs, (0.1, 0.5), 0.0, &mut scratch) {
            results.push((c, est.bpm, est.confidence));
        }
    }
    results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());

    println!("\ntop-10 subcarriers by confidence:");
    for (c, bpm, conf) in results.iter().take(10) {
        println!("  col {:>2}: {:.2} bpm  (confidence {:.3})", c, bpm, conf);
    }

    let k = 10.min(results.len());
    let mut top: Vec<f32> = results.iter().take(k).map(|r| r.1).collect();
    top.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = top[k / 2];
    let best = results[0].1;

    println!("\n=== RESULT ===");
    println!("best (highest-confidence) subcarrier: {:.2} bpm", best);
    println!("median of top-{}: {:.2} bpm", k, median);
    if expected.is_finite() {
        println!("expected: {:.1} bpm", expected);
        println!("error (median): {:.2} bpm  -> {}", (median - expected).abs(),
            if (median - expected).abs() <= 2.0 { "WITHIN ±2 bpm ✓" } else { "OUT OF TOLERANCE ✗" });
    }
}
