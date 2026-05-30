# HOST — I/O Sources & Replay Driver

**Segment prefix:** `HOST-`
**Crate:** `wave-core` (host layer; the only part that does I/O — file reads, later sockets/serial)
**Upstream:** `docs/high-level-design.md` (Approach → swappable I/O hosts)
**Consumes:** `CORE-` (frame parsing for the ESP32 path), `DSP-` primitives (`estimate_rate_bpm`, `hampel`, `resample_uniform`, `estimate_fs`).

## Context and Design Philosophy

`HOST-` is the I/O boundary. The signal core (`CORE-`/`DSP-`) is pure; `HOST-` is where bytes enter the system — from a dataset file today, a live ESP32 stream later. The design goal from the HLD is that **the data source is a config swap, not a rewrite**: a `CsiSource` trait abstracts origin, and a single replay driver runs any source through the DSP pipeline.

Principle: the source owns *carrier selection* (which physical subcarriers are usable for its hardware), so it yields ready-to-process amplitude samples. The driver is source-agnostic — it windows, fuses, and estimates. This keeps the Intel-5300 dataset and a future ESP32 stream behind one interface despite different PHYs.

## The `CsiSource` Trait

```rust
pub trait CsiSource {
    /// Advance to the next sample; `None` at end of stream.
    fn next_sample(&mut self) -> Option<CsiSample<'_>>;
    /// Nominal sampling rate hint (Hz).
    fn nominal_fs(&self) -> f32;
}

pub struct CsiSample<'a> {
    pub timestamp_us: u32,
    /// Usable subcarrier amplitudes (guard/DC already excluded by the source).
    pub amplitudes: &'a [f32],
}
```

A sample is `(timestamp, usable amplitudes)`. The source has already dropped guard/DC carriers appropriate to its PHY — so the driver never needs PHY-specific knowledge.

## Sources

### `CsvReplaySource` (the validated dataset)

Reads the open WiFi-CSI-MiningTool amplitude CSV: **N rows × 90 amplitude columns** (Intel-5300, 30 subcarriers × 3 antennas), one row per packet. The amplitude-only files carry **no timestamps**, so timestamps are synthesized from a configured `Fs` (the recordings are ~25 Hz; confirmed from the companion `AMP_PHASE` files). An optional `label_bpm` carries the filename's ground-truth rate for validation. All 90 columns are presented as usable (the dataset is already amplitude, and validation showed all carriers carry the respiration signal).

### `Esp32ByteSource<I>` (the hardware path)

Adapts an iterator of canonical ESP32 frame byte buffers: each buffer is parsed by `CORE-` (`RawCsiFrame::parse`), guard/DC carriers are dropped via `dsp::is_guard_or_dc` over the `CORE-` index map, amplitudes are computed from the surviving subcarriers, and `timestamp_us` is taken from `CORE-PARSE-010`. This is the path a live ESP32 (or a binary capture) flows through. A frame that fails to parse is skipped.

## Replay Driver

```rust
pub fn replay_estimate(source: &mut dyn CsiSource, cfg: &DspConfig) -> Option<Estimate>
```

Drains up to `cfg.window_len` samples into a column buffer (one column per usable subcarrier, capped at `MAX_SUB`), computes `Fs` from the collected timestamps (`estimate_fs`, falling back to `nominal_fs` if degenerate), then for the top-`K` columns by in-band variance: resample → Hampel → `estimate_rate_bpm`, fusing by highest confidence. Returns the fused `Estimate` or `None` (too few samples, all gaps, or low confidence). This is the offline analog of `DspProcessor`; both share the same `DSP-` primitives (no duplicate FFT/filter math).

## Validation Wiring

`DSP-VAL-001` is re-expressible through `HOST-`: construct a `CsvReplaySource` over each labeled file and assert `replay_estimate(...).bpm` is within ±2 bpm of `label_bpm`. This routes the real dataset through the production `CsiSource` interface rather than ad-hoc CSV parsing in the test.

## Decisions & Alternatives

| Decision | Chosen | Alternatives | Rationale |
|---|---|---|---|
| Sample granularity | `(timestamp, usable amplitudes)` | Raw bytes; full complex CSI | DSP needs amplitude + time; pushing carrier selection into the source keeps the driver PHY-agnostic and unifies Intel-5300 + ESP32. |
| CSV timestamps | Synthesized from configured `Fs` | Require the `AMP_PHASE` timestamped file | The amplitude-only files (smaller, what we fetch) lack timestamps; ~25 Hz is confirmed and stable; the resample stage tolerates the resulting uniform grid. |
| Driver vs `DspProcessor` | Separate offline `replay_estimate` reusing DSP primitives | Force the dataset through `DspProcessor::update` | Avoids re-encoding Intel-5300 amplitudes into ESP32 binary frames (lossy/fake). Both orchestrators share the same primitive functions. |
| ESP32 adapter | Generic over an iterator of byte buffers | Concrete file/socket reader | Keeps `HOST-` testable with in-memory frames now; real serial/UDP is a thin wrapper later. |

## Open Questions & Future Decisions

### Resolved
1. ✅ Source yields pre-filtered usable amplitudes; driver is PHY-agnostic.
2. ✅ CSV timestamps synthesized from `Fs` (amplitude-only files).
3. ✅ **Robustness (I/O boundary — degrade, never panic):** too few rows / empty source → `None`; ragged rows (column count ≠ the first row's) are dropped; non-float tokens are skipped; `Fs ≤ 0` or degenerate timestamps fall back to `nominal_fs()`, and if that is also ≤ 0 → `None`; an ESP32 byte buffer that fails `CORE` parse is skipped and the stream continues; a frame with more usable carriers than `MAX_SUB` is truncated to `MAX_SUB`.

### Deferred
1. **Per-sample belt ground-truth alignment.** The BPM-labeled files make alignment trivial (one label per file). Time-syncing the per-sample accelerometer/belt `S#_GT` traces to CSI (for instantaneous-rate validation) is future work.
2. **Live ESP32 transport** (serial/UDP) — a thin `CsiSource` wrapping the `Esp32ByteSource` adapter; not built now.
3. **Streaming large files** — current `CsvReplaySource` may load lazily line-by-line; full-file vs streaming is a perf detail tuned later.

### Flagged cross-segment
- The `replay_estimate` fusion logic mirrors `DspProcessor`'s; both depend on `DSP-` primitives. If the fusion algorithm changes, update both (a `DSP-` change cascading to both orchestrators).

## References

- HLD `docs/high-level-design.md` — swappable I/O hosts; validation dataset.
- `docs/llds/core.md`, `docs/llds/dsp.md` — consumed interfaces.
- WiFi-CSI-MiningTool dataset format — `scripts/fetch_dataset.sh`.
