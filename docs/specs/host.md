# HOST — EARS Specifications

Segment: `HOST-` · LLD: `docs/llds/host.md` · HLD: swappable I/O hosts.

Status: `[x]` implemented · `[ ]` active gap · `[D]` deferred. All `HOST-` specs implemented in `crates/wave-core/src/host.rs` with passing `@spec` tests; `HOST-DRV-005` validated on the real dataset.

## CsiSource Interface

- [x] **HOST-SRC-001**: The `CsiSource` trait shall yield successive samples, each carrying a microsecond timestamp and a slice of usable subcarrier amplitudes (guard/DC carriers already excluded by the source), and shall report `None` at end of stream.
- [x] **HOST-SRC-002**: A `CsiSource` shall expose a nominal sampling-rate hint in Hz.

## CSV Replay Source

- [x] **HOST-CSV-001**: `CsvReplaySource` shall parse a CSI amplitude CSV of one row per packet with a fixed number of amplitude columns, presenting each row's amplitudes as one sample.
- [x] **HOST-CSV-002**: When the amplitude file carries no timestamps, `CsvReplaySource` shall synthesize per-sample timestamps from the configured sampling rate.
- [x] **HOST-CSV-003**: If a CSV row's column count differs from the first row's, then `CsvReplaySource` shall skip that row.
- [x] **HOST-CSV-004**: If a CSV token is not a valid float, then `CsvReplaySource` shall skip that token without panicking.
- [x] **HOST-CSV-005**: `CsvReplaySource` shall carry an optional ground-truth label (breaths per minute) for validation.

## ESP32 Byte Source

- [x] **HOST-ESP-001**: `Esp32ByteSource` shall adapt an iterator of canonical ESP32 frame byte buffers into `CsiSample`s, parsing each buffer via `CORE-`, excluding guard/DC carriers via the `DSP-` classification, and taking the timestamp from `CORE-PARSE-010`.
- [x] **HOST-ESP-002**: If an ESP32 byte buffer fails to parse, then `Esp32ByteSource` shall skip it and continue the stream.
- [x] **HOST-ESP-003**: If a frame yields more usable carriers than the maximum tracked, then `Esp32ByteSource` shall truncate to that maximum.

## Replay Driver

- [x] **HOST-DRV-001**: `replay_estimate` shall drain up to `window_len` samples, derive `Fs` from the collected timestamps (falling back to the source's nominal rate if degenerate), and produce a fused respiration `Estimate` using the `DSP-` primitives.
- [x] **HOST-DRV-002**: When fusing, `replay_estimate` shall select the top-`K` columns by in-band variance and combine their per-column estimates by highest confidence.
- [x] **HOST-DRV-003**: If the source provides fewer than `window_len` samples, then `replay_estimate` shall return `None`.
- [x] **HOST-DRV-004**: If the derived and nominal sampling rates are both non-positive, then `replay_estimate` shall return `None`.
- [x] **HOST-DRV-005**: On the labeled validation dataset, `replay_estimate` over a `CsvReplaySource` shall recover a breathing rate within ±2 bpm of the file's label (the `HOST-` realization of `DSP-VAL-001`).
