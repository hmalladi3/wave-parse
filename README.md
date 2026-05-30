# wave-parse

A zero-copy WiFi **Channel State Information (CSI)** parsing engine and respiration-sensing pipeline in idiomatic Rust — with an authenticated telemetry path. It turns the per-subcarrier amplitude ripple a breathing body imprints on WiFi into a breaths-per-minute estimate, validated against real labeled CSI recordings.

It is a **clean-room reimplementation** of the CSI sensing node from the viral [RuView](https://github.com/ruvnet/RuView) project, built from RuView's public quality/security audit as a spec — deliberately *not* coupled to its internals, and fixing the exact failures the audit found.

## Result

Run against real Linux 802.11n CSI Tool recordings (30 subcarriers × 3 antennas, ~25 Hz), recovered breathing rate vs. the labeled ground truth:

| Ground truth | Recovered | Error |
|---|---|---|
| 9 bpm  | 9.03  | 0.03 |
| 12 bpm | 12.02 | 0.02 |
| 15 bpm | 15.02 | 0.02 |
| 18 bpm | 17.96 | 0.04 |
| 21 bpm | 20.98 | 0.02 |

All errors **< 0.05 bpm** — comfortably inside the ±2 bpm target.

## Try it (2 commands)

```bash
# 1. fetch the open, BPM-labeled CSI dataset (~25 MB)
bash scripts/fetch_dataset.sh

# 2. recover the breathing rate from a 15-bpm recording
cargo run --release --example validate_breathing -- data/breathing/s10_15bpm_amp.csv 25.0 15
```

```
loaded 7495 rows × 90 cols, Fs=25 Hz
window N=4096 (163.8 s)
...
best (highest-confidence) subcarrier: 15.03 bpm
median of top-10: 15.02 bpm
expected: 15.0 bpm
error (median): 0.02 bpm  -> WITHIN ±2 bpm ✓
```

Or run the whole suite (the dataset-validation tests activate automatically once the data is present):

```bash
cargo test            # 68 tests
cargo clippy --all-targets
```

## Architecture

A single `no_std`-friendly core library with thin host shells — the signal logic is pure and I/O-free, so the data source is a config swap (dataset today, an ESP32 stream later).

```
wave-core
├── CORE   zero-copy CSI frame parser · lazy subcarrier extraction · lock-free SPSC ring
├── DSP    Hampel outlier filter · timestamp resampling · windowed real-FFT · harmonic guard
├── HOST   CsiSource trait · CSV replay · ESP32 byte adapter · replay/fusion driver
└── SEC    HMAC-SHA256 authenticated telemetry (ring)
```

Each segment is designed → specified → tested → implemented under a linked-intent workflow: every behavior is an [EARS spec](docs/specs/) traced from a [design doc](docs/llds/), and every spec is cited by the code and a test that proves it (`// @spec` annotations). See [`docs/high-level-design.md`](docs/high-level-design.md).

## Engineering highlights

- **Auditable `unsafe`** — zero-copy parsing over raw byte buffers where every `unsafe` block carries a `// SAFETY:` justification, enforced by a test and a `cargo-fuzz` target. (RuView's audit found 324 undocumented `unsafe` blocks.)
- **Lock-free SPSC ring** — sequence-numbered, drop-oldest under overflow with a visible drop counter; zero heap allocation on the hot path (proven by a counting-allocator test).
- **Real DSP** — Hampel (median/MAD) spike rejection, timestamp resampling that tolerates dropped frames, Hann-windowed radix-2 FFT, parabolic peak interpolation, and a harmonic guard so a 2× harmonic isn't mistaken for the fundamental. Returns `None` rather than a fabricated rate when unsure.
- **Real cryptography** — telemetry authenticated with `ring`-backed HMAC-SHA256 and constant-time verification; tampered payloads, wrong keys, and truncated messages are all rejected. (RuView's audit found a fake HMAC providing zero protection.)

## Scope & honesty

- Validated on **Intel-5300** CSI (the open [WiFi-CSI-MiningTool](https://github.com/AlbanyArmenta0711/WiFi-CSI-MiningTool) dataset). ESP32-S3 is the intended *hardware* target via a future `HOST` adapter; the DSP is source-agnostic.
- The pipeline is proven **correct** on real labeled data; tuning constants (`DspConfig`) only carry meaning against a given capture setup.
- Telemetry is authenticated, not encrypted — the threat model is tampering/forgery, not eavesdropping.

## License

MIT OR Apache-2.0
