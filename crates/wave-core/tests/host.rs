//! HOST sources + replay driver. See docs/specs/host.md.

mod common;
use common::*;
use std::f32::consts::PI;
use wave_core::dsp::{DspConfig, MAX_SUBCARRIERS};
use wave_core::host::*;

// @spec HOST-CSV-001, HOST-SRC-001
#[test]
fn csv_source_yields_rows_as_samples() {
    let mut s = CsvReplaySource::from_str("1,2,3\n4,5,6\n", 25.0);
    let first = { let a = s.next_sample().unwrap(); a.amplitudes.to_vec() };
    assert_eq!(first, vec![1.0, 2.0, 3.0]);
    let second = { let b = s.next_sample().unwrap(); b.amplitudes.to_vec() };
    assert_eq!(second, vec![4.0, 5.0, 6.0]);
    assert!(s.next_sample().is_none());
}

// @spec HOST-CSV-002, HOST-SRC-002
#[test]
fn csv_synthesizes_increasing_timestamps_from_fs() {
    let mut s = CsvReplaySource::from_str("1,2\n3,4\n5,6\n", 25.0);
    assert_eq!(s.nominal_fs(), 25.0);
    let t0 = { s.next_sample().unwrap().timestamp_us };
    let t1 = { s.next_sample().unwrap().timestamp_us };
    let t2 = { s.next_sample().unwrap().timestamp_us };
    assert!(t1 > t0 && t2 > t1, "timestamps must increase");
    // ~1/25 s = 40000 us spacing.
    assert!((t1 - t0) as i64 >= 30_000 && (t1 - t0) as i64 <= 50_000);
}

// @spec HOST-CSV-003
#[test]
fn csv_skips_ragged_rows() {
    // Middle row has a different column count → dropped.
    let mut s = CsvReplaySource::from_str("1,2,3\n4,5\n7,8,9\n", 25.0);
    let mut count = 0;
    while { let got = s.next_sample().is_some(); got } {
        count += 1;
    }
    assert_eq!(count, 2);
}

// @spec HOST-CSV-004
#[test]
fn csv_skips_non_float_tokens() {
    // A bad token shrinks that row → it becomes ragged and is dropped; clean rows survive.
    let s = CsvReplaySource::from_str("10,20,30\n40,bad,60\n70,80,90\n", 25.0);
    assert_eq!(s.len(), 2);
}

// @spec HOST-CSV-005
#[test]
fn csv_carries_optional_label() {
    let s = CsvReplaySource::from_str("1,2,3\n", 25.0).with_label(15.0);
    assert_eq!(s.label_bpm(), Some(15.0));
}

// @spec HOST-ESP-001
#[test]
fn esp32_source_parses_frames_to_samples() {
    // 30 carriers so several survive guard/DC exclusion.
    let pairs: Vec<(i8, i8)> = (0..30).map(|i| (0i8, (10 + i) as i8)).collect();
    let frames: Vec<Vec<u8>> =
        (0..3u32).map(|i| FrameBuilder::new().timestamp(i * 1000).pairs(&pairs).build()).collect();
    let mut src = Esp32ByteSource::new(frames.into_iter(), 25.0);
    let s = src.next_sample().unwrap();
    assert_eq!(s.timestamp_us, 0);
    assert!(!s.amplitudes.is_empty(), "non-guard carriers should yield amplitudes");
}

// @spec HOST-ESP-002
#[test]
fn esp32_source_skips_unparseable_buffers() {
    let pairs: Vec<(i8, i8)> = (0..30).map(|i| (0i8, (10 + i) as i8)).collect();
    let good = FrameBuilder::new().timestamp(7777).pairs(&pairs).build();
    let frames: Vec<Vec<u8>> = vec![vec![0u8; 3], good]; // first is too short to parse
    let mut src = Esp32ByteSource::new(frames.into_iter(), 25.0);
    let s = src.next_sample().expect("should skip the bad buffer and return the good one");
    assert_eq!(s.timestamp_us, 7777);
}

// @spec HOST-ESP-003
#[test]
fn esp32_source_truncates_to_max_subcarriers() {
    let pairs: Vec<(i8, i8)> = (0..60).map(|i| (0i8, (1 + i % 100) as i8)).collect();
    let frames: Vec<Vec<u8>> = vec![FrameBuilder::new().pairs(&pairs).build()];
    let mut src = Esp32ByteSource::new(frames.into_iter(), 25.0);
    let s = src.next_sample().unwrap();
    assert!(s.amplitudes.len() <= MAX_SUBCARRIERS);
}

/// Build a CSV of `rows` packets × `cols` columns, each a sinusoid at `bpm`.
fn synth_csv(rows: usize, cols: usize, fs: f32, bpm: f32) -> String {
    let f = bpm / 60.0;
    let mut out = String::new();
    for i in 0..rows {
        let v = 20.0 + 8.0 * (2.0 * PI * f * i as f32 / fs).sin();
        let line: Vec<String> = (0..cols).map(|_| format!("{v:.4}")).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

// @spec HOST-DRV-001, HOST-DRV-002
#[test]
fn replay_estimate_recovers_rate_through_source() {
    let csv = synth_csv(512, 8, 25.0, 15.0);
    let mut src = CsvReplaySource::from_str(&csv, 25.0);
    let est = replay_estimate(&mut src, &DspConfig::default()).expect("estimate");
    assert!((est.bpm - 15.0).abs() <= 2.0, "got {}", est.bpm);
}

// @spec HOST-DRV-003
#[test]
fn replay_estimate_none_when_too_few_samples() {
    let csv = synth_csv(50, 8, 25.0, 15.0); // < default window_len (256)
    let mut src = CsvReplaySource::from_str(&csv, 25.0);
    assert!(replay_estimate(&mut src, &DspConfig::default()).is_none());
}

// @spec HOST-DRV-004
#[test]
fn replay_estimate_none_when_fs_unavailable() {
    let csv = synth_csv(512, 8, 25.0, 15.0);
    let mut src = CsvReplaySource::from_str(&csv, 0.0); // fs hint 0 + synthesized ts all 0
    assert!(replay_estimate(&mut src, &DspConfig::default()).is_none());
}

// @spec HOST-DRV-005
#[test]
fn replay_estimate_validates_on_dataset() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/breathing");
    let cases = [(9.0, "s10_9bpm_amp.csv"), (12.0, "s10_12bpm_amp.csv"), (15.0, "s10_15bpm_amp.csv"), (18.0, "s10_18bpm_amp.csv"), (21.0, "s10_21bpm_amp.csv")];
    // Respiration at 25 Hz needs a long window for frequency resolution
    // (~164 s); the 256-sample default is for the synthetic processor path.
    let cfg = DspConfig { window_len: 4096, ..DspConfig::default() };
    let mut ran = 0;
    for (expected, file) in cases {
        let path = format!("{dir}/{file}");
        if std::fs::metadata(&path).is_err() {
            continue;
        }
        let mut src = CsvReplaySource::from_file(&path, 25.0).unwrap().with_label(expected);
        let est = replay_estimate(&mut src, &cfg).expect("estimate");
        assert!((est.bpm - expected).abs() <= 2.0, "{file}: recovered {:.2}, expected {expected}", est.bpm);
        ran += 1;
    }
    if ran == 0 {
        eprintln!("HOST-DRV-005: dataset absent — run scripts/fetch_dataset.sh");
    }
}
