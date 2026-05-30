//! `HOST-` — I/O sources and the offline replay driver. See `docs/llds/host.md`.
//!
//! This is the only module that performs I/O. Sources yield ready-to-process
//! amplitude samples (guard/DC excluded by the source); `replay_estimate` is the
//! offline analog of `DspProcessor`, sharing the same `DSP-` primitives.

use crate::dsp::{
    estimate_fs, estimate_rate_bpm, hampel, is_guard_or_dc, resample_uniform, DspConfig, DspError,
    Estimate, FftScratch, MAX_SUBCARRIERS,
};
use crate::RawCsiFrame;

/// One time-stamped sample of usable subcarrier amplitudes.
pub struct CsiSample<'a> {
    pub timestamp_us: u32,
    pub amplitudes: &'a [f32],
}

/// A swappable source of CSI samples (dataset replay today, ESP32 stream later).
pub trait CsiSource {
    /// Advance to the next sample; `None` at end of stream.
    fn next_sample(&mut self) -> Option<CsiSample<'_>>;
    /// Nominal sampling-rate hint (Hz).
    fn nominal_fs(&self) -> f32;
}

/// Replays a CSI amplitude CSV (one row per packet, fixed column count).
pub struct CsvReplaySource {
    rows: Vec<Vec<f32>>,
    fs: f32,
    label_bpm: Option<f32>,
    pos: usize,
}

impl CsvReplaySource {
    // @spec HOST-CSV-001, HOST-CSV-003, HOST-CSV-004
    pub fn from_str(text: &str, fs: f32) -> Self {
        let parsed: Vec<Vec<f32>> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split(',').filter_map(|t| t.trim().parse::<f32>().ok()).collect::<Vec<f32>>())
            .collect();
        // The first non-empty row sets the expected width; ragged rows (including
        // rows shrunk by a dropped non-float token) are discarded.
        let width = parsed.iter().find(|r| !r.is_empty()).map(|r| r.len()).unwrap_or(0);
        let rows = parsed.into_iter().filter(|r| width > 0 && r.len() == width).collect();
        CsvReplaySource { rows, fs, label_bpm: None, pos: 0 }
    }

    pub fn from_file(path: &str, fs: f32) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_str(&text, fs))
    }

    pub fn with_label(mut self, bpm: f32) -> Self {
        self.label_bpm = Some(bpm);
        self
    }

    // @spec HOST-CSV-005
    pub fn label_bpm(&self) -> Option<f32> {
        self.label_bpm
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl CsiSource for CsvReplaySource {
    // @spec HOST-SRC-001, HOST-CSV-002
    fn next_sample(&mut self) -> Option<CsiSample<'_>> {
        if self.pos >= self.rows.len() {
            return None;
        }
        let idx = self.pos;
        self.pos += 1;
        // Amplitude-only files carry no timestamps → synthesize from Fs.
        let timestamp_us = if self.fs > 0.0 {
            (idx as f64 * 1e6 / self.fs as f64) as u32
        } else {
            0
        };
        Some(CsiSample { timestamp_us, amplitudes: &self.rows[idx] })
    }

    // @spec HOST-SRC-002
    fn nominal_fs(&self) -> f32 {
        self.fs
    }
}

/// Adapts an iterator of canonical ESP32 frame byte buffers into samples.
pub struct Esp32ByteSource<I> {
    iter: I,
    fs: f32,
    amps: Vec<f32>,
}

impl<I> Esp32ByteSource<I> {
    pub fn new(iter: I, fs: f32) -> Self {
        Esp32ByteSource { iter, fs, amps: Vec::with_capacity(MAX_SUBCARRIERS) }
    }
}

impl<I: Iterator<Item = Vec<u8>>> CsiSource for Esp32ByteSource<I> {
    // @spec HOST-SRC-001, HOST-ESP-001, HOST-ESP-002, HOST-ESP-003
    fn next_sample(&mut self) -> Option<CsiSample<'_>> {
        loop {
            let buf = self.iter.next()?;
            let frame = match RawCsiFrame::parse(&buf) {
                Ok(f) => f,
                Err(_) => continue, // skip unparseable buffers, keep streaming
            };
            self.amps.clear();
            for (k, sc) in frame.subcarriers().enumerate() {
                if self.amps.len() >= MAX_SUBCARRIERS {
                    break; // truncate to the tracked maximum
                }
                if let Some(idx) = frame.subcarrier_index(k) {
                    if !is_guard_or_dc(idx) {
                        self.amps.push(sc.amplitude());
                    }
                }
            }
            let timestamp_us = frame.timestamp_us();
            return Some(CsiSample { timestamp_us, amplitudes: &self.amps });
        }
    }

    // @spec HOST-SRC-002
    fn nominal_fs(&self) -> f32 {
        self.fs
    }
}

/// Drain up to `cfg.window_len` samples from a source and produce a fused
/// respiration estimate using the `DSP-` primitives.
// @spec HOST-DRV-001, HOST-DRV-002, HOST-DRV-003, HOST-DRV-004, HOST-DRV-005
pub fn replay_estimate(source: &mut dyn CsiSource, cfg: &DspConfig) -> Option<Estimate> {
    let wl = cfg.window_len;
    let mut times: Vec<u32> = Vec::with_capacity(wl);
    let mut cols: Vec<Vec<f32>> = Vec::new();
    let mut n_sub = 0usize;

    while times.len() < wl {
        let s = match source.next_sample() {
            Some(s) => s,
            None => break,
        };
        if cols.is_empty() {
            n_sub = s.amplitudes.len().min(MAX_SUBCARRIERS);
            if n_sub == 0 {
                break;
            }
            cols = (0..n_sub).map(|_| Vec::with_capacity(wl)).collect();
        }
        times.push(s.timestamp_us);
        for (c, col) in cols.iter_mut().enumerate() {
            col.push(s.amplitudes.get(c).copied().unwrap_or(0.0));
        }
    }

    if times.len() < wl || n_sub == 0 {
        return None; // HOST-DRV-003
    }

    let mut fs = estimate_fs(&times);
    if fs <= 0.0 {
        fs = source.nominal_fs();
    }
    if fs <= 0.0 {
        return None; // HOST-DRV-004
    }

    // Rank columns by variance, take the top K.
    let mut ranked: Vec<(usize, f32)> = (0..n_sub)
        .map(|c| {
            let col = &cols[c];
            let mean = col.iter().sum::<f32>() / col.len() as f32;
            let v = col.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>();
            (c, v)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    let kk = cfg.k_subcarriers.min(n_sub);

    let mut scratch = FftScratch::new(wl);
    let mut grid = vec![0.0f32; wl];
    let mut best: Option<Estimate> = None;

    for &(c, _) in ranked.iter().take(kk) {
        let n = match resample_uniform(&times, &cols[c], fs, cfg.max_gap_s, &mut grid) {
            Ok(n) => n,
            Err(DspError::GapTooLong) => return None, // window invalid
            Err(_) => continue,
        };
        grid[n..wl].fill(0.0);
        hampel(&mut grid[..wl], cfg.hampel_half_win, cfg.hampel_k);
        if let Some(est) = estimate_rate_bpm(&grid[..wl], fs, cfg.band_hz, cfg.confidence_threshold, &mut scratch) {
            if best.is_none_or(|b| est.confidence > b.confidence) {
                best = Some(est);
            }
        }
    }
    best
}
