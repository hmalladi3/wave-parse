//! DSP pipeline behavior, validated on synthetic signals. See docs/specs/dsp.md.
//!
//! Ground-truth validation against the real dataset (DSP-VAL-001) is a separate,
//! `#[ignore]`d test that requires the download; these tests prove the math.

mod common;
use common::*;
use std::f32::consts::PI;
use wave_core::dsp::*;
use wave_core::RawCsiFrame;

const FS: f32 = 28.0;
const N: usize = 256;

/// Sinusoid of `f_hz` sampled at `fs`, offset to stay positive.
fn sinusoid(n: usize, fs: f32, f_hz: f32, amp: f32, offset: f32) -> Vec<f32> {
    (0..n).map(|i| offset + amp * (2.0 * PI * f_hz * i as f32 / fs).sin()).collect()
}

fn bpm_to_hz(bpm: f32) -> f32 {
    bpm / 60.0
}

// @spec DSP-FFT-002, DSP-FFT-003, DSP-OUT-001, DSP-OUT-002
#[test]
fn recovers_known_breathing_rate() {
    let series = sinusoid(N, FS, bpm_to_hz(15.0), 1.0, 0.0);
    let mut scratch = FftScratch::new(N);
    let est = estimate_rate_bpm(&series, FS, (0.1, 0.5), 0.1, &mut scratch).expect("should detect a peak");
    assert!((est.bpm - 15.0).abs() <= 2.0, "expected ~15 bpm, got {}", est.bpm);
    assert!(est.confidence > 0.1);
}

// @spec DSP-OUT-006
#[test]
fn flat_signal_yields_no_estimate() {
    let series = vec![0.0f32; N];
    let mut scratch = FftScratch::new(N);
    assert!(estimate_rate_bpm(&series, FS, (0.1, 0.5), 0.15, &mut scratch).is_none());
}

// @spec DSP-OUT-003
#[test]
fn harmonic_guard_picks_fundamental() {
    // Fundamental 12 bpm (0.2 Hz) + weaker 2nd harmonic 24 bpm (0.4 Hz).
    let f0 = bpm_to_hz(12.0);
    let series: Vec<f32> = (0..N)
        .map(|i| {
            let t = i as f32 / FS;
            (2.0 * PI * f0 * t).sin() + 0.5 * (2.0 * PI * 2.0 * f0 * t).sin()
        })
        .collect();
    let mut scratch = FftScratch::new(N);
    let est = estimate_rate_bpm(&series, FS, (0.1, 0.5), 0.1, &mut scratch).unwrap();
    assert!((est.bpm - 12.0).abs() <= 2.0, "harmonic guard failed: got {}", est.bpm);
}

// @spec DSP-FILT-001
#[test]
fn hampel_replaces_spikes() {
    let mut series = sinusoid(N, FS, bpm_to_hz(15.0), 1.0, 0.0);
    series[100] = 1000.0; // injected spike
    series[150] = -1000.0;
    hampel(&mut series, 3, 3.0);
    assert!(series[100].abs() < 5.0, "spike not removed: {}", series[100]);
    assert!(series[150].abs() < 5.0, "spike not removed: {}", series[150]);
}

// @spec DSP-FILT-002
#[test]
fn hampel_handles_boundaries_without_panic() {
    let mut series = sinusoid(16, FS, bpm_to_hz(15.0), 1.0, 0.0);
    hampel(&mut series, 5, 3.0); // half-window larger than edge distance
}

// @spec DSP-FILT-003
#[test]
fn breathing_signal_survives_filtering() {
    // Spike-corrupted breathing signal: after Hampel, rate is still recoverable.
    let mut series = sinusoid(N, FS, bpm_to_hz(18.0), 1.0, 0.0);
    for &i in &[40usize, 41, 130, 200] {
        series[i] = 500.0;
    }
    hampel(&mut series, 3, 3.0);
    let mut scratch = FftScratch::new(N);
    let est = estimate_rate_bpm(&series, FS, (0.1, 0.5), 0.1, &mut scratch).unwrap();
    assert!((est.bpm - 18.0).abs() <= 2.0, "got {}", est.bpm);
}

// @spec DSP-FFT-001
#[test]
fn hann_window_is_zero_at_edges_one_at_center() {
    let mut w = vec![0.0f32; 64];
    hann_window(&mut w);
    assert!(w[0].abs() < 1e-6);
    assert!(w[63].abs() < 1e-3);
    assert!((w[32] - 1.0).abs() < 0.02);
}

// @spec DSP-RESAMP-002
#[test]
fn estimate_fs_from_timestamps() {
    // ~28 Hz → ~35714 us spacing.
    let step = (1_000_000.0 / FS) as u32;
    let times: Vec<u32> = (0..50).map(|i| i * step).collect();
    let fs = estimate_fs(&times);
    assert!((fs - FS).abs() < 1.0, "got {}", fs);
}

// @spec DSP-RESAMP-001, DSP-RESAMP-003
#[test]
fn resample_recovers_rate_from_jittered_timestamps() {
    let step = 1_000_000.0 / FS;
    let f0 = bpm_to_hz(15.0);
    let mut times = Vec::new();
    let mut samples = Vec::new();
    for i in 0..N {
        // Non-uniform timing: small deterministic jitter, no large gaps.
        let jitter = if i % 2 == 0 { 0.15 } else { -0.15 } * step;
        let t_us = (i as f32 * step + jitter) as u32;
        times.push(t_us);
        samples.push((2.0 * PI * f0 * (t_us as f32 / 1_000_000.0)).sin());
    }
    let mut grid = vec![0.0f32; N];
    let n = resample_uniform(&times, &samples, FS, 1.0, &mut grid).unwrap();
    let mut scratch = FftScratch::new(N);
    let est = estimate_rate_bpm(&grid[..n.min(N)], FS, (0.1, 0.5), 0.1, &mut scratch).unwrap();
    assert!((est.bpm - 15.0).abs() <= 2.0, "got {}", est.bpm);
}

// @spec DSP-RESAMP-004
#[test]
fn resample_rejects_overlong_gap() {
    let step = (1_000_000.0 / FS) as u32;
    // A 5-second hole in the middle.
    let times = vec![0, step, 2 * step, 2 * step + 5_000_000, 2 * step + 5_000_000 + step];
    let samples = vec![0.0, 0.1, 0.2, 0.3, 0.4];
    let mut out = vec![0.0f32; 1024];
    assert_eq!(resample_uniform(&times, &samples, FS, 1.0, &mut out), Err(DspError::GapTooLong));
}

// @spec DSP-RESAMP-005
#[test]
fn resample_treats_non_finite_as_gap() {
    let step = (1_000_000.0 / FS) as u32;
    let times: Vec<u32> = (0..10).map(|i| i * step).collect();
    let mut samples = vec![0.0f32; 10];
    for (i, s) in samples.iter_mut().enumerate() {
        *s = i as f32;
    }
    samples[5] = f32::NAN;
    let mut out = vec![0.0f32; 64];
    let n = resample_uniform(&times, &samples, FS, 1.0, &mut out).unwrap();
    assert!(out[..n].iter().all(|v| v.is_finite()), "NaN propagated into output");
}

// @spec DSP-PHASE-001
#[test]
fn sanitize_phase_removes_linear_trend() {
    // Linear phase ramp across subcarriers (slope 0.3, intercept 1.0) + tiny ripple.
    let mut phases: Vec<f32> = (0..52).map(|k| 0.3 * k as f32 + 1.0 + 0.01 * (k as f32).sin()).collect();
    sanitize_phase(&mut phases);
    // After removing the linear fit, residual should be small.
    let max = phases.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!(max < 0.1, "linear trend not removed, residual max {}", max);
}

// @spec DSP-GATE-002
#[test]
fn dc_carrier_is_classified_as_guard() {
    assert!(is_guard_or_dc(0), "DC carrier (index 0) must be excluded");
    assert!(!is_guard_or_dc(-10), "a normal data carrier must not be excluded");
}

// @spec DSP-VAL-002
#[test]
fn validation_invalidated_by_core_drops() {
    assert!(validation_run_is_valid(0));
    assert!(!validation_run_is_valid(1));
}

// ---- DspProcessor (end-to-end over frames) ----

/// Build a frame at timestamp `ts_us` whose subcarriers all encode amplitude `amp`.
fn amp_frame(ts_us: u32, amp: u8, rssi: i8, n_sub: usize) -> Vec<u8> {
    let pairs: Vec<(i8, i8)> = (0..n_sub).map(|_| (0i8, amp as i8)).collect();
    FrameBuilder::new().timestamp(ts_us).rssi(rssi).pairs(&pairs).build()
}

// @spec DSP-OUT-004
#[test]
fn processor_cold_start_returns_none() {
    let mut p = DspProcessor::new(DspConfig::default());
    let step = (1_000_000.0 / FS) as u32;
    for i in 0..10u32 {
        let buf = amp_frame(i * step, 20, -50, 8);
        p.update(&RawCsiFrame::parse(&buf).unwrap());
    }
    assert!(p.estimate().is_none(), "should be None before the window fills");
}

// @spec DSP-OUT-005, DSP-GATE-001, DSP-GATE-004
#[test]
fn processor_all_gated_returns_none() {
    let cfg = DspConfig::default();
    let mut p = DspProcessor::new(cfg);
    let step = (1_000_000.0 / FS) as u32;
    for i in 0..300u32 {
        // rssi far below the floor → every frame gated.
        let buf = amp_frame(i * step, 20, -120, 8);
        p.update(&RawCsiFrame::parse(&buf).unwrap());
    }
    assert!(p.estimate().is_none());
    assert!(p.gated_frames() > 0, "gated frames should be counted");
}

// @spec DSP-OUT-007, DSP-PHASE-002, DSP-GATE-003
#[test]
fn processor_end_to_end_recovers_rate() {
    let mut p = DspProcessor::new(DspConfig::default());
    let step = 1_000_000.0 / FS;
    let f0 = bpm_to_hz(15.0);
    for i in 0..512u32 {
        let amp = (20.0 + 8.0 * (2.0 * PI * f0 * i as f32 / FS).sin()) as u8;
        let buf = amp_frame((i as f32 * step) as u32, amp, -50, 8);
        p.update(&RawCsiFrame::parse(&buf).unwrap());
    }
    let est = p.estimate().expect("should produce an estimate once warm");
    assert!((est.bpm - 15.0).abs() <= 2.0, "got {}", est.bpm);
    assert!(!p.waveform().is_empty());
}

// @spec DSP-OUT-008
#[test]
fn processor_resets_on_layout_change() {
    let mut p = DspProcessor::new(DspConfig::default());
    let step = (1_000_000.0 / FS) as u32;
    for i in 0..300u32 {
        p.update(&RawCsiFrame::parse(&amp_frame(i * step, 20, -50, 8)).unwrap());
    }
    // Subcarrier count changes → window reset → not enough fresh frames → None.
    let buf = amp_frame(300 * step, 20, -50, 16);
    p.update(&RawCsiFrame::parse(&buf).unwrap());
    assert!(p.estimate().is_none(), "layout change should reset the window");
}

/// Recover the breathing rate from a 90-column CSI-amplitude CSV: condition each
/// column (Hampel), estimate per column, fuse the top-10 by confidence (median).
fn recover_bpm_from_csv(text: &str, fs: f32, win: usize) -> f32 {
    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect())
        .collect();
    let cols = rows[0].len();
    let mut scratch = FftScratch::new(win);
    let mut res: Vec<(f32, f32)> = Vec::new();
    #[allow(clippy::needless_range_loop)] // column index also selects rows[r][c]
    for c in 0..cols {
        let mut s: Vec<f32> = (0..win).map(|r| rows[r][c]).collect();
        hampel(&mut s, 5, 3.0);
        if let Some(e) = estimate_rate_bpm(&s, fs, (0.1, 0.5), 0.0, &mut scratch) {
            res.push((e.bpm, e.confidence));
        }
    }
    res.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let k = 10.min(res.len());
    let mut top: Vec<f32> = res.iter().take(k).map(|r| r.0).collect();
    top.sort_by(|a, b| a.partial_cmp(b).unwrap());
    top[k / 2]
}

// @spec DSP-VAL-001
#[test]
fn validates_within_two_bpm_on_dataset() {
    // Runs against the open WiFi-CSI-MiningTool dataset if present
    // (scripts/fetch_dataset.sh); skips gracefully otherwise.
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/breathing");
    let fs = 25.0f32;
    let cases = [(9.0, "s10_9bpm_amp.csv"), (12.0, "s10_12bpm_amp.csv"), (15.0, "s10_15bpm_amp.csv"), (18.0, "s10_18bpm_amp.csv"), (21.0, "s10_21bpm_amp.csv")];
    let mut ran = 0;
    for (expected, file) in cases {
        let Ok(text) = std::fs::read_to_string(format!("{dir}/{file}")) else { continue };
        let bpm = recover_bpm_from_csv(&text, fs, 4096);
        assert!((bpm - expected).abs() <= 2.0, "{file}: recovered {bpm:.2} bpm, expected {expected}");
        ran += 1;
    }
    if ran == 0 {
        eprintln!("DSP-VAL-001: dataset absent — run scripts/fetch_dataset.sh to enable real validation");
    }
}
