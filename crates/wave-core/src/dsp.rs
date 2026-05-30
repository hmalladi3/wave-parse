//! `DSP-` — respiration signal extraction. See `docs/llds/dsp.md` and
//! `docs/specs/dsp.md`. Pure functions plus a stateful `DspProcessor`.
//!
//! The reference implementation favors clarity and correctness; SIMD is a
//! follow-up applied behind these scalar references (HLD: correctness first).

use crate::RawCsiFrame;
use std::f32::consts::PI;

/// Max subcarriers tracked per frame/window.
pub const MAX_SUBCARRIERS: usize = 64;
const MAX_SUB: usize = MAX_SUBCARRIERS;

/// Errors from the resampling stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspError {
    /// A gap between samples exceeded `max_gap_s` (window must be invalidated).
    GapTooLong,
    /// Output buffer length inconsistent with the requested grid.
    LengthMismatch,
}

/// Tunable pipeline parameters. The LLD fixes the shape; these constants are
/// tuned against the dataset during implementation.
#[derive(Debug, Clone)]
pub struct DspConfig {
    pub fs_hint: f32,
    pub window_len: usize,
    pub k_subcarriers: usize,
    pub hampel_half_win: usize,
    pub hampel_k: f32,
    pub band_hz: (f32, f32),
    pub max_gap_s: f32,
    pub rssi_floor: i8,
    pub noise_ceiling: i8,
    pub confidence_threshold: f32,
}

impl Default for DspConfig {
    fn default() -> Self {
        DspConfig {
            fs_hint: 28.0,
            window_len: 256,
            k_subcarriers: 4,
            hampel_half_win: 3,
            hampel_k: 3.0,
            band_hz: (0.1, 0.5),
            max_gap_s: 1.0,
            rssi_floor: -85,
            noise_ceiling: -60,
            confidence_threshold: 0.15,
        }
    }
}

/// A respiration-rate estimate plus its confidence (in-band peak / total power).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub bpm: f32,
    pub confidence: f32,
}

/// Fill `out` with a Hann window.
// @spec DSP-FFT-001
pub fn hann_window(out: &mut [f32]) {
    let n = out.len();
    if n <= 1 {
        out.iter_mut().for_each(|v| *v = 1.0);
        return;
    }
    let denom = (n - 1) as f32;
    for (i, v) in out.iter_mut().enumerate() {
        *v = 0.5 * (1.0 - (2.0 * PI * i as f32 / denom).cos());
    }
}

/// In-place Hampel outlier filter (sliding median + MAD). Window statistics are
/// taken from the original (pre-filter) values to avoid cascading replacements.
// @spec DSP-FILT-001, DSP-FILT-002
pub fn hampel(series: &mut [f32], half_win: usize, k: f32) {
    let n = series.len();
    const MAX: usize = 4096;
    if n == 0 || half_win == 0 || n > MAX {
        return;
    }
    let mut orig = [0f32; MAX];
    orig[..n].copy_from_slice(series);
    let hw = half_win.min(256);
    let mut win = [0f32; 513];
    let mut dev = [0f32; 513];
    let cmp = |a: &f32, b: &f32| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal);
    for i in 0..n {
        let lo = i.saturating_sub(hw);
        let hi = (i + hw + 1).min(n);
        let w = hi - lo;
        win[..w].copy_from_slice(&orig[lo..hi]);
        win[..w].sort_unstable_by(cmp);
        let med = win[w / 2];
        for j in 0..w {
            dev[j] = (win[j] - med).abs();
        }
        dev[..w].sort_unstable_by(cmp);
        let mad = 1.4826 * dev[w / 2];
        if mad > 0.0 && (orig[i] - med).abs() > k * mad {
            series[i] = med;
        }
    }
}

/// Unwrap phase and remove a least-squares linear fit across subcarrier index
/// (SpotFi sanitization).
// @spec DSP-PHASE-001
pub fn sanitize_phase(phases: &mut [f32]) {
    let n = phases.len();
    if n == 0 {
        return;
    }
    for i in 1..n {
        while phases[i] - phases[i - 1] > PI {
            phases[i] -= 2.0 * PI;
        }
        while phases[i] - phases[i - 1] < -PI {
            phases[i] += 2.0 * PI;
        }
    }
    let nf = n as f32;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for (i, &y) in phases.iter().enumerate() {
        let x = i as f32;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let denom = nf * sxx - sx * sx;
    let (m, b) = if denom.abs() > 1e-12 {
        let m = (nf * sxy - sx * sy) / denom;
        (m, (sy - m * sx) / nf)
    } else {
        (0.0, sy / nf)
    };
    for (i, p) in phases.iter_mut().enumerate() {
        *p -= m * i as f32 + b;
    }
}

/// Estimate sampling rate (Hz) from the median inter-sample interval.
// @spec DSP-RESAMP-002
pub fn estimate_fs(times_us: &[u32]) -> f32 {
    if times_us.len() < 2 {
        return 0.0;
    }
    let mut buf = [0u32; 4096];
    let m = (times_us.len() - 1).min(4096);
    for i in 0..m {
        buf[i] = times_us[i + 1].saturating_sub(times_us[i]);
    }
    buf[..m].sort_unstable();
    let med = buf[m / 2] as f32;
    if med <= 0.0 {
        0.0
    } else {
        1e6 / med
    }
}

/// Index of the next finite sample at or after `from`, within `..=last`.
fn next_finite(samples: &[f32], from: usize, last: usize) -> usize {
    let mut i = from;
    while i <= last && !samples[i].is_finite() {
        i += 1;
    }
    i
}

/// Resample `(times_us, samples)` onto a uniform grid at `fs` into `out`.
/// Linearly interpolates gaps up to `max_gap_s`; non-finite samples are treated
/// as gaps; a gap beyond `max_gap_s` returns `Err(GapTooLong)`.
// @spec DSP-RESAMP-001, DSP-RESAMP-003, DSP-RESAMP-004, DSP-RESAMP-005
pub fn resample_uniform(
    times_us: &[u32],
    samples: &[f32],
    fs: f32,
    max_gap_s: f32,
    out: &mut [f32],
) -> Result<usize, DspError> {
    let n = times_us.len();
    if n != samples.len() {
        return Err(DspError::LengthMismatch);
    }
    if n == 0 || fs <= 0.0 {
        return Ok(0);
    }
    let max_gap_us = (max_gap_s * 1e6) as i64;
    let mut last_t: Option<i64> = None;
    for i in 0..n {
        if samples[i].is_finite() {
            let t = times_us[i] as i64;
            if let Some(lt) = last_t {
                if t - lt > max_gap_us {
                    return Err(DspError::GapTooLong);
                }
            }
            last_t = Some(t);
        }
    }
    let first = next_finite(samples, 0, n - 1);
    if first > n - 1 {
        return Ok(0);
    }
    let last = {
        let mut i = n - 1;
        while !samples[i].is_finite() {
            i -= 1;
        }
        i
    };
    let t0 = times_us[first] as f64;
    let t_end = times_us[last] as f64;
    let dt = 1e6 / fs as f64;
    let count_full = ((t_end - t0) / dt).floor() as usize + 1;
    let count = count_full.min(out.len());
    let mut lo = first;
    for (g, slot) in out.iter_mut().enumerate().take(count) {
        let t = t0 + g as f64 * dt;
        loop {
            let nx = next_finite(samples, lo + 1, last);
            if nx <= last && (times_us[nx] as f64) <= t {
                lo = nx;
            } else {
                break;
            }
        }
        let hi = next_finite(samples, lo + 1, last);
        *slot = if hi > last {
            samples[lo]
        } else {
            let tl = times_us[lo] as f64;
            let th = times_us[hi] as f64;
            let frac = if th > tl { ((t - tl) / (th - tl)) as f32 } else { 0.0 };
            samples[lo] + frac * (samples[hi] - samples[lo])
        };
    }
    Ok(count)
}

/// True if a physical subcarrier index is a guard, null, or DC carrier.
// @spec DSP-GATE-002
pub fn is_guard_or_dc(physical_index: i32) -> bool {
    physical_index == 0 || physical_index.abs() >= 28
}

/// Whether a validation run is valid given `CORE`'s overflow drop count.
// @spec DSP-VAL-002
pub fn validation_run_is_valid(core_dropped_frames: u64) -> bool {
    core_dropped_frames == 0
}

/// Preallocated FFT working buffers (constructed once; the hot path is alloc-free).
pub struct FftScratch {
    n: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    win: Vec<f32>,
}

impl FftScratch {
    /// `n` must be a power of two.
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two(), "FFT length must be a power of two");
        let mut win = vec![0.0f32; n];
        hann_window(&mut win);
        FftScratch { n, re: vec![0.0; n], im: vec![0.0; n], win }
    }
}

/// In-place iterative radix-2 Cooley-Tukey FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0usize;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let a = i + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Windowed-FFT respiration-rate estimator. Returns `None` when in-band
/// confidence is below `confidence_threshold`. Applies the harmonic guard
/// (lowest-frequency strong local-max peak is the fundamental) and refines the
/// peak with parabolic interpolation for sub-bin frequency resolution.
// @spec DSP-FFT-002, DSP-FFT-003, DSP-OUT-001, DSP-OUT-002, DSP-OUT-003, DSP-OUT-006
#[allow(clippy::needless_range_loop)] // parallel indexing of series/re/im/win by one index
pub fn estimate_rate_bpm(
    series: &[f32],
    fs: f32,
    band_hz: (f32, f32),
    confidence_threshold: f32,
    scratch: &mut FftScratch,
) -> Option<Estimate> {
    let n = scratch.n;
    let len = series.len().min(n);
    if len == 0 || fs <= 0.0 {
        return None;
    }
    let mean = series[..len].iter().sum::<f32>() / len as f32;
    for i in 0..len {
        scratch.re[i] = (series[i] - mean) * scratch.win[i];
        scratch.im[i] = 0.0;
    }
    for i in len..n {
        scratch.re[i] = 0.0;
        scratch.im[i] = 0.0;
    }
    fft(&mut scratch.re, &mut scratch.im);

    let mag = |k: usize| (scratch.re[k] * scratch.re[k] + scratch.im[k] * scratch.im[k]).sqrt();
    let bin_lo = ((band_hz.0 * n as f32) / fs).ceil() as usize;
    let bin_hi = (((band_hz.1 * n as f32) / fs).floor() as usize).min(n / 2 - 1);
    if bin_lo < 1 || bin_lo > bin_hi {
        return None;
    }

    let mut total = 0.0f32;
    let mut peak = 0.0f32;
    for k in bin_lo..=bin_hi {
        let mg = mag(k);
        total += mg * mg;
        if mg > peak {
            peak = mg;
        }
    }
    if total <= 1e-12 || peak <= 0.0 {
        return None;
    }

    // Harmonic guard: lowest-frequency strong local maximum.
    let mut k0 = None;
    for k in bin_lo..=bin_hi {
        let mg = mag(k);
        if mg >= 0.5 * peak {
            let left = if k > bin_lo { mag(k - 1) } else { 0.0 };
            let right = if k < bin_hi { mag(k + 1) } else { 0.0 };
            if mg >= left && mg >= right {
                k0 = Some(k);
                break;
            }
        }
    }
    let k0 = k0?;

    // Parabolic interpolation for sub-bin frequency.
    let (a, b, c) = (mag(k0 - 1), mag(k0), mag(k0 + 1));
    let denom = a - 2.0 * b + c;
    let delta = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
    let freq = (k0 as f32 + delta) * fs / n as f32;
    let confidence = (b * b) / total;
    if confidence < confidence_threshold {
        return None;
    }
    Some(Estimate { bpm: freq * 60.0, confidence })
}

/// Stateful per-session processor: ingest frames, emit respiration estimates.
pub struct DspProcessor {
    config: DspConfig,
    window_len: usize,
    n_sub: usize,
    count: usize,
    head: usize,
    gated: u64,
    times: Vec<u32>,
    amps: Vec<f32>, // window_len * MAX_SUB, row-major by frame slot
    row: Vec<f32>,  // scratch: this frame's non-guard amplitudes
    col: Vec<f32>,
    times_ord: Vec<u32>,
    grid: Vec<f32>,
    waveform: Vec<f32>,
    fft: FftScratch,
}

impl DspProcessor {
    pub fn new(config: DspConfig) -> Self {
        let wl = config.window_len;
        let fft = FftScratch::new(wl);
        DspProcessor {
            window_len: wl,
            n_sub: 0,
            count: 0,
            head: 0,
            gated: 0,
            times: vec![0; wl],
            amps: vec![0.0; wl * MAX_SUB],
            row: vec![0.0; MAX_SUB],
            col: vec![0.0; wl],
            times_ord: vec![0; wl],
            grid: vec![0.0; wl],
            waveform: vec![0.0; wl],
            fft,
            config,
        }
    }

    /// Ingest one CSI frame (gating + window accumulation). Default extraction
    /// is amplitude (no phase sanitization).
    // @spec DSP-GATE-001, DSP-GATE-003, DSP-OUT-008, DSP-PHASE-002
    pub fn update(&mut self, frame: &RawCsiFrame) {
        if frame.rssi() < self.config.rssi_floor || frame.noise_floor() > self.config.noise_ceiling {
            self.gated += 1;
            return;
        }
        // Collect non-guard subcarrier amplitudes.
        let mut cur_n = 0usize;
        for (k, sc) in frame.subcarriers().enumerate() {
            if cur_n >= MAX_SUB {
                break;
            }
            match frame.subcarrier_index(k) {
                Some(idx) if !is_guard_or_dc(idx) => {
                    self.row[cur_n] = sc.amplitude();
                    cur_n += 1;
                }
                _ => {}
            }
        }
        if cur_n == 0 {
            self.gated += 1;
            return;
        }
        // Session layout change → reset (DSP-OUT-008).
        if self.n_sub == 0 {
            self.n_sub = cur_n;
        } else if cur_n != self.n_sub {
            self.reset();
            self.n_sub = cur_n;
        }
        let h = self.head;
        self.times[h] = frame.timestamp_us();
        let base = h * MAX_SUB;
        self.amps[base..base + self.n_sub].copy_from_slice(&self.row[..self.n_sub]);
        self.head = (h + 1) % self.window_len;
        self.count = (self.count + 1).min(self.window_len);
    }

    /// Current estimate, or `None` on cold start / all-gated / low-confidence / long-gap.
    // @spec DSP-OUT-004, DSP-OUT-005, DSP-OUT-007
    #[allow(clippy::needless_range_loop)] // circular-buffer indexing by computed slot
    pub fn estimate(&mut self) -> Option<Estimate> {
        if self.count < self.window_len || self.n_sub == 0 {
            return None;
        }
        let wl = self.window_len;
        for f in 0..wl {
            self.times_ord[f] = self.times[(self.head + f) % wl];
        }
        let fs = estimate_fs(&self.times_ord[..wl]);
        if fs <= 0.0 {
            return None;
        }

        // Rank subcarriers by variance, select the top K.
        let mut var = [0f32; MAX_SUB];
        for s in 0..self.n_sub {
            let mut mean = 0.0;
            for f in 0..wl {
                mean += self.amps[((self.head + f) % wl) * MAX_SUB + s];
            }
            mean /= wl as f32;
            let mut v = 0.0;
            for f in 0..wl {
                let d = self.amps[((self.head + f) % wl) * MAX_SUB + s] - mean;
                v += d * d;
            }
            var[s] = v;
        }
        let kk = self.config.k_subcarriers.min(self.n_sub);

        let mut best: Option<Estimate> = None;
        for _rank in 0..kk {
            // Pick the highest-variance not-yet-used subcarrier.
            let mut s_best = usize::MAX;
            let mut v_best = -1.0f32;
            for s in 0..self.n_sub {
                if var[s] > v_best {
                    v_best = var[s];
                    s_best = s;
                }
            }
            if s_best == usize::MAX {
                break;
            }
            var[s_best] = -1.0; // mark used

            for f in 0..wl {
                self.col[f] = self.amps[((self.head + f) % wl) * MAX_SUB + s_best];
            }
            let n = match resample_uniform(&self.times_ord[..wl], &self.col[..wl], fs, self.config.max_gap_s, &mut self.grid) {
                Ok(n) => n,
                Err(DspError::GapTooLong) => return None, // window invalid
                Err(_) => continue,
            };
            for g in n..wl {
                self.grid[g] = 0.0;
            }
            hampel(&mut self.grid[..wl], self.config.hampel_half_win, self.config.hampel_k);
            if let Some(est) = estimate_rate_bpm(
                &self.grid[..wl],
                fs,
                self.config.band_hz,
                self.config.confidence_threshold,
                &mut self.fft,
            ) {
                if best.is_none_or(|b| est.confidence > b.confidence) {
                    best = Some(est);
                    self.waveform[..wl].copy_from_slice(&self.grid[..wl]);
                }
            }
        }
        best
    }

    /// Last computed breathing waveform (the conditioned series).
    // @spec DSP-FILT-003
    pub fn waveform(&self) -> &[f32] {
        &self.waveform[..self.window_len]
    }

    /// Frames excluded by quality gating (distinct from `CORE` overflow drops).
    // @spec DSP-GATE-004
    pub fn gated_frames(&self) -> u64 {
        self.gated
    }

    /// Clear the window (session reset).
    // @spec DSP-OUT-008
    pub fn reset(&mut self) {
        self.count = 0;
        self.head = 0;
        self.n_sub = 0;
    }
}
