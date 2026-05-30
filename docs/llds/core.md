# CORE — CSI Frame Types, Zero-Copy Parser & Frame Ring

**Segment prefix:** `CORE-`
**Crate:** `wave-core` (lib, `no_std`-friendly, no I/O)
**Upstream:** `docs/high-level-design.md` (Approach → core mechanism; Decisions #1, #4, #6)

## Context and Design Philosophy

`CORE-` owns the lowest layer of `wave-core`: the in-memory representation of a CSI frame, the zero-copy parser that turns a raw byte buffer into a typed, bounds-checked view, the lazy extraction of per-subcarrier amplitude/phase, and the fixed-capacity ring that hands frames from a producer thread to the DSP consumer without per-frame allocation.

Guiding principles, inherited from the HLD:

1. **Zero heap allocation in the steady-state hot path** (HLD Goal 2). Parsing borrows; it never owns or copies the payload. The ring is preallocated.
2. **Auditable `unsafe`** (HLD Decision #4). Every `unsafe` block carries a `// SAFETY:` proof obligation discharged against an explicitly stated invariant. This is the deliberate antithesis of RuView's 324 undocumented blocks.
3. **Panic-free on adversarial input** (HLD Goal 4). Malformed, truncated, or hostile frames return typed errors. The parser is a fuzz target; a panic is a test failure.
4. **No physics here.** `CORE-` exposes raw complex subcarriers and their amplitude/phase. Outlier rejection, phase sanitization, and FFT are `DSP-`'s job. The boundary is: `CORE-` produces clean *numbers from bytes*; `DSP-` produces *meaning from numbers*.

## Canonical Frame Representation

The canonical raw frame is the **ESP-IDF `wifi_csi_info_t` in-memory layout** — the exact bytes a real ESP32-S3 host hands over. Choosing the hardware's own representation (rather than a bespoke format) means the future `wave-esp32` host feeds `CORE-` directly, and the `wave-replay` host's only job is to *reconstruct* these bytes from the dataset's CSV (a `HOST-` concern — see Open Questions).

Logical layout of a frame buffer (`&[u8]`, little-endian, native ESP32 order):

| Region | Contents | Notes |
|---|---|---|
| `rx_ctrl` | radio metadata (rssi, rate, sig_mode, mcs, bandwidth, channel, noise_floor, timestamp, …) | `CORE-` parses only the fields it needs (see below); the rest is skipped, not copied |
| `mac[6]` | source MAC of the CSI packet | exposed as a borrowed `&[u8; 6]` |
| `first_word_invalid` | flag: first complex pair group is RX-state garbage | honored during subcarrier iteration |
| `len` | CSI payload length in bytes | drives bounds validation |
| CSI payload | interleaved `int8` pairs `[imag, real]` per subcarrier | the zero-copy target |

**Fields `CORE-` actually reads:** `sig_mode`/`bandwidth` (to select the subcarrier index map), `first_word_invalid`, `len`, `mac`, `rssi`/`noise_floor` (passed through for DSP gating), and `local_timestamp` (the ESP-IDF capture timestamp in **microseconds**, exposed as a `u32` for `DSP-` resampling — `DSP-RESAMP-001/002`). All other `rx_ctrl` fields are addressable but not interpreted — minimizing the parse surface and the `unsafe` footprint.

## Zero-Copy Parsing

`RawCsiFrame<'a>` is a typed view holding `&'a [u8]` plus precomputed, validated offsets. Construction is the only fallible step:

```
RawCsiFrame::parse(buf: &'a [u8]) -> Result<RawCsiFrame<'a>, FrameError>
```

`parse` validates *before* any unchecked access: buffer length covers the fixed header, `len` field is consistent with the remaining buffer, and the payload is a whole number of complex pairs. Only after every bound is proven does the accessor layer use `unsafe` to read fixed-offset fields, each with a `// SAFETY:` note citing the invariant `parse` established. The invariant ("offsets O..O+k are in-bounds because `parse` checked `buf.len() >= header_end` and `header_end + len <= buf.len()`") is stated once at the type and referenced by each block.

**Alignment.** Frame slabs in the ring are **64-byte aligned** so that `DSP-` can run SIMD over extracted samples without re-copying — 64 bytes covers AVX-512 and a full cache line, so it is wide enough regardless of which SIMD width `DSP-` ends up using (a deliberate over-provision to eliminate a class of cross-segment alignment bugs at negligible padding cost). The alignment guarantee is a `CORE-` invariant **serving `DSP-`** — a cross-segment interface, flagged below.

## Subcarrier Extraction

The CSI payload is interleaved `[imag, real]` `int8` pairs. `RawCsiFrame::subcarriers()` returns a lazy iterator yielding `Subcarrier { imag: i8, real: i8 }`, computing nothing until pulled. Two derived accessors, also lazy and allocation-free:

- `amplitude()` → `sqrt(real² + imag²)` per subcarrier.
- `phase()` → `atan2(imag, real)` per subcarrier.

Behavior is defined for the awkward cases (see edge audit): when `first_word_invalid` is set, the **first 4 bytes (= 2 complex pairs)** are skipped — this is the ESP-IDF "first word" semantics, not a single pair — and a `(0, 0)` pair yields amplitude `0.0` and phase `0.0` (atan2(0,0) is defined to 0 rather than left to platform NaN behavior). The subcarrier **index map** (which physical subcarrier each pair corresponds to, including guard/null/DC carriers) is selected from `bandwidth`/`sig_mode`. The map is defined over the **post-skip** yielded subcarriers and accounts for the skip, so subcarrier *k* always reports its true physical carrier identity regardless of whether `first_word_invalid` trimmed the leading pairs. `CORE-` exposes raw subcarriers in capture order and provides the index map, but does **not** drop guard/DC carriers — that filtering is a `DSP-` decision.

## Frame Ring

`FrameRing<const N: usize>` is a fixed-capacity, lock-free **single-producer / single-consumer** ring of 64-byte-aligned frame slots. The producer (a `HOST-` reader thread) claims a slot, writes raw bytes into it, and publishes; the consumer (the DSP thread) borrows the slot, processes, and releases. No allocation after construction.

- **Memory ordering:** publish via `Release`, observe via `Acquire`, so the consumer sees fully-written bytes.
- **Single-producer / single-consumer is an invariant, not a runtime check** — matches the system shape (one CSI source → one DSP pipeline). Multi-producer is out of scope (a mesh concern, deferred per HLD Non-Goals).
- **Overflow policy: drop-oldest via sequence numbers (disruptor-style).** Naive "overwrite the oldest" would require the producer to advance the *consumer's* read index — that breaks pure SPSC and can corrupt a slot the consumer is mid-read on. Instead each slot carries a monotonic publish **sequence number**: the producer only ever advances its *own* write sequence (overwriting the slot `N` positions back), and the consumer detects that it has been *lapped* by comparing the slot's sequence against its own expected sequence. On a lap, the consumer skips forward to the oldest still-valid slot and adds the gap to an atomic `dropped_frames` counter. This keeps the structure genuinely lock-free SPSC, never tears a frame, and bounds latency to the freshest `N` frames. Loss is bounded *and* visible, never silent. `DSP-` therefore treats its input as a stream that *may contain gaps* and must tolerate non-contiguous windows.
- **`dropped_frames` counts overflow only.** It is incremented solely on ring lapping. Any later `DSP-`-side quality gating (e.g. low-RSSI frames discarded after pop, per `CORE-PARSE-009`) is a *separate* `DSP-` counter — conflating the two would make the telemetry counter misreport ingest health. Gating happens consumer-side, after pop, so a gated frame never affects the ring.
- **Empty poll:** the consumer's `pop` is non-blocking and returns `None` when empty; the DSP thread idles rather than spins-with-side-effects.
- **Alignment on ingest:** if the producer's source bytes are not 64-byte aligned, the slot copy-aligns them once on write (outside the DSP hot path), so consumers always get aligned frames for SIMD.

## Error Model

`FrameError` is a non-allocating enum: `TooShortForHeader`, `LenExceedsBuffer`, `OddPayloadLength`, `EmptyPayload`, `UnknownBandwidth`. Parsing returns these; it never panics, asserts, or indexes unchecked. This is what makes the `cargo-fuzz` target (HLD Goal 4) meaningful.

## Decisions & Alternatives

| Decision | Chosen | Alternatives Considered | Rationale |
|---|---|---|---|
| Frame access model | Zero-copy `RawCsiFrame<'a>` borrowed view | Owned `CsiFrame` struct (copy on parse) | HLD Decision #4 — zero-alloc hot path + auditable `unsafe`. Copying would defeat both. |
| Canonical byte layout | ESP-IDF `wifi_csi_info_t` in-memory layout | Bespoke wire format; raw int8 payload only | Real hardware emits this; `wave-esp32` becomes a direct swap and the dataset host's job is just reconstruction. |
| Subcarrier amp/phase | Lazy iterator, computed on pull | Precompute into a fixed array on parse | Lazy = zero-alloc and DSP pulls only what it windows. A persistent per-subcarrier time buffer is `DSP-`'s ring, not `CORE-`'s frame ring. |
| Producer→consumer handoff | Lock-free SPSC fixed ring | Mutex queue; `std::sync::mpsc`; crossbeam channel | SPSC matches one-source→one-DSP, is zero-alloc, `no_std`-compatible, and showcases lock-free Rust. mpsc allocates and is multi-producer overkill. |
| Failure handling | `Result<_, FrameError>`, panic-free | `assert!`/`panic!`; `debug_assert!` | Goal 4 fuzzing requires the parser to treat all byte sequences as input, not as bugs. |
| Guard/DC subcarrier filtering | Not in `CORE-` (exposed raw + index map) | Filter in `CORE-` | Which carriers are usable is a DSP/physics decision; `CORE-` stays mechanism-only. |

## Open Questions & Future Decisions

### Resolved
1. ✅ Canonical representation = ESP-IDF layout (Decisions table).
2. ✅ Parsing is panic-free and returns typed errors (serves Goal 4).
3. ✅ Guard/DC filtering belongs to `DSP-`, not `CORE-`.
4. ✅ Ring overflow = drop-oldest/keep-newest with an atomic `dropped_frames` counter (Frame Ring section). DSP consumes a possibly-gapped stream.
5. ✅ `first_word_invalid` with payload < 4 bytes → `parse` still succeeds; `subcarriers()` yields an **empty iterator** (the skip consumes the whole payload). The `EmptyPayload` *error* is reserved for a frame whose declared `len` is 0.
6. ✅ Empty-ring poll returns `None` (non-blocking).
7. ✅ Unaligned input is copy-aligned into the slot on ingest (off the hot path).
8. ✅ SPSC is a documented contract, not a runtime check (multi-producer is a deferred mesh concern).

### Deferred
1. **Subcarrier count & index map per bandwidth/sig_mode.** ESP32 emits different subcarrier counts for LLTF / HT-LTF / 20 vs 40 MHz. `CORE-` reads the mode and selects the map rather than hardcoding; the concrete table is built during implementation against ESP-IDF headers + the pinned dataset's actual captures. `UnknownBandwidth` covers modes outside the table.

### Flagged cross-segment (pause before cascading)
- **Dataset CSV → canonical binary reconstruction** is a `HOST-` concern. `CORE-` defines the target byte layout; `HOST-` (`wave-replay`) maps the ESP32-CSI-Tool CSV columns into it. Do not design that mapping in this segment.
- **64-byte alignment guarantee** is consumed by `DSP-` SIMD (wide enough for AVX-512). Owned here, depended on there.
- **Validation-integrity rule (carry into `HOST-` and `DSP-`):** drop-oldest is correct for a live ESP32 (airtime cannot be paused), but during a *dataset validation run* — the basis for HLD Goal 1's ±2 bpm — silently dropped frames would confound the breathing-rate comparison against ground truth. Therefore `wave-replay` must pace to real-time (or slower), and **any nonzero `dropped_frames` during a validation run invalidates that run** (treated as a test failure). The live-hardware policy and the validation-integrity rule coexist; they are not in conflict.

## References

- ESP-IDF Wi-Fi CSI API — `wifi_csi_info_t`, `wifi_pkt_rx_ctrl_t`, `first_word_invalid` semantics.
- ESP32-CSI-Tool CSV schema (used by the pinned validation datasets) — for the `HOST-` reconstruction mapping.
- HLD `docs/high-level-design.md` — Decisions #1, #4, #6; Goals 2, 4.
