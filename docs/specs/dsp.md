# DSP — EARS Specifications

Segment: `DSP-` · LLD: `docs/llds/dsp.md` · HLD goal served: 1 (respiration rate within ±2 bpm of ground truth).

Status: `[x]` implemented · `[ ]` active gap · `[D]` deferred. All `DSP-` specs are implemented in `crates/wave-core/src/dsp.rs` with passing `@spec` tests; `DSP-VAL-001` is validated on the open WiFi-CSI-MiningTool dataset.

## Quality Gating & Subcarrier Selection

- [x] **DSP-GATE-001**: If a frame's `rssi` is below the configured floor or its `noise_floor` is above the configured ceiling, then the system shall exclude that frame from the window and increment a `DSP-`-side `gated_frames` counter.
- [x] **DSP-GATE-002**: When selecting subcarriers, the system shall exclude guard, null, and DC carriers identified via the `CORE-` subcarrier index map.
- [x] **DSP-GATE-003**: When building a window, the system shall select the top-`K` remaining subcarriers ranked by in-band variance.
- [x] **DSP-GATE-004**: The `gated_frames` counter (signal-quality drops) shall be distinct from `CORE-`'s `dropped_frames` counter (ring overflow), per `CORE-RING-008`.

## Resampling

- [x] **DSP-RESAMP-001**: When forming a window for spectral analysis, the system shall resample the timestamped samples onto a uniform time grid at sampling rate `Fs`.
- [x] **DSP-RESAMP-002**: When determining `Fs`, the system shall compute it from the median inter-frame interval of the window rather than a hardcoded constant.
- [x] **DSP-RESAMP-003**: When the gap between consecutive samples is at most `max_gap`, the system shall linearly interpolate across the gap.
- [x] **DSP-RESAMP-004**: If a gap between consecutive samples exceeds `max_gap`, then the system shall invalidate and clear the current window and emit no estimate until the window refills.
- [x] **DSP-RESAMP-005**: If a sample is non-finite (`NaN`/`Inf`), then the system shall treat it as a gap sample and shall not propagate it to the FFT.

## Phase Sanitization (variant-scoped)

- [x] **DSP-PHASE-001**: Where the phase extraction path is enabled, the system shall unwrap each frame's per-subcarrier phase and remove a least-squares linear fit across subcarrier index (SpotFi sanitization).
- [x] **DSP-PHASE-002**: Where the amplitude extraction path is used (the default), the system shall not apply phase sanitization.

## Conditioning Filters

- [x] **DSP-FILT-001**: The system shall apply a Hampel filter (sliding-window median + MAD, default `k = 3`) to each selected subcarrier series, replacing any sample deviating more than `k·MAD` from the local median with the local median.
- [x] **DSP-FILT-002**: When the Hampel window extends past a series boundary, the system shall use a truncated/asymmetric window without out-of-range access.
- [x] **DSP-FILT-003**: The system shall detrend and bandpass each conditioned series to the respiration band 0.1–0.5 Hz; the bandpassed series is the emitted breathing waveform.

## Spectral Estimation

- [x] **DSP-FFT-001**: Before the FFT, the system shall apply a Hann window to the conditioned series.
- [x] **DSP-FFT-002**: The system shall compute a real-FFT and locate the peak bin within the respiration band 0.1–0.5 Hz (bounds inclusive).
- [x] **DSP-FFT-003**: The system shall convert the selected peak bin frequency to breaths per minute.

## Fusion & Output

- [x] **DSP-OUT-001**: When producing a rate estimate, the system shall fuse the `K` selected subcarriers' in-band spectra (sum of band power) into a single estimate.
- [x] **DSP-OUT-002**: The system shall compute a confidence value as the in-band peak power divided by the total in-band power.
- [x] **DSP-OUT-003**: When resolving the respiration rate, the system shall select the lowest-frequency strong in-band peak as the fundamental (harmonic guard), so a 2× respiration harmonic or heartbeat component is not reported as the breathing rate.
- [x] **DSP-OUT-004**: While the window is not yet full (cold start), the system shall return no estimate.
- [x] **DSP-OUT-005**: If all subcarriers in the window are gated out, then the system shall return no estimate.
- [x] **DSP-OUT-006**: If the in-band confidence is below the configured threshold, then the system shall return no estimate (it shall not emit a fabricated rate).
- [x] **DSP-OUT-007**: When an estimate is produced, the system shall return the breaths-per-minute value, the confidence, and the breathing waveform.
- [x] **DSP-OUT-008**: If the per-frame subcarrier count changes within a session, then the system shall reset the window.
- [x] **DSP-OUT-009**: During per-frame update and per-window estimation, the system shall perform no heap allocation (all scratch preallocated at `DspProcessor` construction).

## Validation

- [x] **DSP-VAL-001**: On the validation dataset, the system's breathing-rate estimate shall be within ±2 bpm of the ground-truth respiration rate. *(Validated on the open WiFi-CSI-MiningTool dataset, subject S10, labels 9/12/15/18/21 bpm at Fs≈25 Hz: recovered 9.03/12.02/15.02/17.96/20.98 bpm — all errors < 0.05 bpm.)*
- [x] **DSP-VAL-002**: If a validation run observes `CORE-` `dropped_frames > 0`, then the run shall be invalidated and treated as a test failure.
