# wave-parse

A high-throughput, **zero-copy WiFi Channel State Information (CSI) parsing engine and DSP pipeline** written in idiomatic Rust, with an authenticated telemetry path. Clean-room reimplementation of the CSI sensing node popularized by [RuView](https://github.com/ruvnet/RuView), built from RuView's public [Quality Engineering audit](https://gist.github.com/proffesor-for-testing/02321e3f272720aa94484fffec6ab19b) as a spec — deliberately **not** API-coupled to RuView's internals.

## What it is

`wave-parse` ingests CSI frames (per-subcarrier amplitude + phase), cleans them through a documented DSP pipeline (Hampel outlier rejection → phase sanitization → FFT band extraction), and emits an authenticated respiration waveform / vitals estimate. The signal logic lives in a pure, I/O-free core library that is fuzzed, benchmarked, and `no_std`-friendly; thin host binaries supply the I/O (a public-dataset replayer now; a real ESP32-S3 stream later).

## What it is NOT

- Not a neural-network / pose-estimation / "see skeletons through walls" system. No ML training.
- Not a drop-in replacement for, or API-compatible with, RuView's components.
- Not a multistatic mesh-fusion engine (single-receiver for now).
- Not a medical device — respiration extraction is validated against dataset ground truth, not for clinical use.

## Architecture (Option C: library-core + thin host shells)

```
wave-core (lib, no I/O, no_std-friendly)   ← parsing + DSP, zero-copy, audited unsafe
   ├── wave-replay (bin)                    ← feeds public CSI datasets through the core
   ├── wave-server (bin)                    ← authenticated telemetry server (HMAC-SHA256)
   └── wave-esp32  (bin, later)             ← real-hardware host; same core, different I/O
```

## Linked-Intent Development (MANDATORY)

**Consult the `linked-intent-dev` skill for ALL code changes.** Changes flow one direction:

```
HLD → LLDs → EARS → Tests → Code
```

Stop after each phase for user review. Mutation, not accumulation — docs reflect current intent, not history.

### Navigation

| What you need | Where |
|---|---|
| High-level design | `docs/high-level-design.md` |
| Low-level designs | `docs/llds/` |
| EARS specs | `docs/specs/` |

### Arrow segments (by layer; EARS prefixes)

| Prefix | Segment |
|---|---|
| `CORE-` | CSI frame types + zero-copy parser + buffers |
| `DSP-` | Signal processing (Hampel, phase, FFT, respiration) |
| `HOST-` | I/O hosts (source trait, dataset replayer, ESP32) |
| `SEC-` | Authenticated telemetry (HMAC-SHA256, transport auth) |

### Code annotations

Annotate code and tests with `@spec` comments citing EARS IDs, placed at the entry point of the behavior's implementation graph:

```rust
// @spec CORE-001, CORE-002
```

## LID Mode: Full
