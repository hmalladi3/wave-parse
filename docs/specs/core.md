# CORE — EARS Specifications

Segment: `CORE-` · LLD: `docs/llds/core.md` · HLD goals served: 2 (zero-alloc hot path), 3 (audited unsafe), 4 (fuzzed parser).

Status: `[x]` implemented · `[ ]` active gap · `[D]` deferred. All `CORE-` specs are implemented in `crates/wave-core` with passing `@spec` tests.

## Frame Parsing

- [x] **CORE-PARSE-001**: When parsing a raw CSI frame buffer, the system shall validate that the buffer length covers the fixed header before accessing any field.
- [x] **CORE-PARSE-002**: If the buffer is shorter than the fixed header, then the system shall return `TooShortForHeader` without reading the payload.
- [x] **CORE-PARSE-003**: If the declared CSI payload length exceeds the bytes remaining after the header, then the system shall return `LenExceedsBuffer`.
- [x] **CORE-PARSE-004**: If the CSI payload length is not an even number of bytes (an incomplete `[imag, real]` pair), then the system shall return `OddPayloadLength`.
- [x] **CORE-PARSE-005**: If the frame's bandwidth/sig_mode is not present in the supported subcarrier-map table, then the system shall return `UnknownBandwidth`.
- [x] **CORE-PARSE-006**: When parsing succeeds, the system shall produce a borrowed zero-copy view over the input buffer, performing no heap allocation and no copy of the CSI payload.
- [x] **CORE-PARSE-007**: The system shall report every parse failure via a `FrameError` value and shall never panic, assert, or perform unchecked indexing on any byte input (including adversarial/fuzzed input).
- [x] **CORE-PARSE-008**: Every `unsafe` block in the parser shall carry a `// SAFETY:` justification referencing the bounds invariant established during parsing.
- [x] **CORE-PARSE-009**: When parsing succeeds, the system shall expose the frame's source MAC, RSSI, and noise floor via borrowed read accessors (no copy of the payload) so the `DSP-` segment can gate on signal quality.
- [x] **CORE-PARSE-010**: When parsing succeeds, the system shall expose the frame's capture timestamp in microseconds (ESP-IDF `local_timestamp`) so the `DSP-` segment can resample onto a uniform time grid.

## Subcarrier Extraction

- [x] **CORE-SUB-001**: When subcarriers are requested from a parsed frame, the system shall yield complex `(imag, real)` `int8` pairs lazily, performing no heap allocation.
- [x] **CORE-SUB-002**: When `first_word_invalid` is set, the system shall skip the first 4 bytes (2 complex `[imag, real]` pairs — the ESP-IDF "first word") before yielding subcarriers.
- [x] **CORE-SUB-003**: If `first_word_invalid` is set and the payload is smaller than 4 bytes, then the system shall yield no subcarriers.
- [x] **CORE-SUB-004**: When amplitude is requested for a subcarrier, the system shall compute `sqrt(real² + imag²)`.
- [x] **CORE-SUB-005**: When phase is requested for a subcarrier, the system shall compute `atan2(imag, real)`.
- [x] **CORE-SUB-006**: If a subcarrier pair is `(0, 0)`, then the system shall yield amplitude `0.0` and phase `0.0`, never `NaN`.
- [x] **CORE-SUB-007**: When yielding subcarriers, the system shall expose them in capture order without dropping guard, null, or DC carriers, and shall provide the bandwidth-appropriate subcarrier index map (carrier filtering is owned by the `DSP-` segment, not `CORE-`).
- [x] **CORE-SUB-008**: When `first_word_invalid` has trimmed the leading pairs, the subcarrier index map shall be defined over the post-skip subcarriers such that subcarrier *k* reports its true physical carrier identity.

## Frame Ring

- [x] **CORE-RING-001**: The system shall provide a fixed-capacity single-producer/single-consumer frame ring that performs no heap allocation after construction.
- [x] **CORE-RING-002**: When the producer publishes a frame, the system shall publish with `Release` ordering such that a consumer observing the slot with `Acquire` ordering sees the fully written bytes.
- [x] **CORE-RING-003**: While the ring is full, when the producer writes a new frame, the system shall overwrite the oldest slot by advancing only its own write sequence number (never the consumer's read index), preserving single-producer/single-consumer correctness.
- [x] **CORE-RING-007**: When the consumer detects via sequence-number mismatch that it has been lapped, the system shall skip forward to the oldest still-valid slot and add the number of skipped frames to the atomic `dropped_frames` counter, without ever reading a torn (partially-overwritten) frame.
- [x] **CORE-RING-008**: The `dropped_frames` counter shall count ring-overflow drops only; any `DSP-`-side quality gating (per `CORE-PARSE-009`) shall use a separate counter and shall not affect the ring.
- [x] **CORE-RING-004**: When the consumer pops from an empty ring, the system shall return `None` without blocking.
- [x] **CORE-RING-005**: If a frame's source bytes are not 64-byte aligned, then the system shall copy-align them into the ring slot on write so that consumers receive 64-byte-aligned frames (64 bytes covers AVX-512 SIMD used by `DSP-`).
- [x] **CORE-RING-006**: The system shall expose the `dropped_frames` count so that frame loss is observable in telemetry and never silent.
