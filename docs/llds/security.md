# SEC — Authenticated Telemetry

**Segment prefix:** `SEC-`
**Crate:** `wave-core` (host-side security layer; never in the pure signal core)
**Upstream:** `docs/high-level-design.md` (Approach → authenticated telemetry; Decision #3)
**Consumes:** `DSP-`/`HOST-` `Estimate`; the `CORE` `dropped_frames` and `DSP` `gated_frames` counters.

## Context and Design Philosophy

Telemetry leaving the node must be **authentic and tamper-evident**. This segment is the direct rebuttal to RuView's audited failures: a *fake HMAC providing zero cryptographic protection* and *unauthenticated wire formats*. `SEC-` replaces that with real **HMAC-SHA256 via the `ring` crate** (a vetted, constant-time implementation) — no hand-rolled crypto.

**Threat model (scoped deliberately):** the threat is a rogue node injecting or tampering with telemetry. The guarantee is **integrity + authenticity** (a receiver sharing the key detects any modification or forgery). Confidentiality is *not* a goal — telemetry (a breathing rate + counters) is not secret; encryption/TLS for transport is a separate, deferred concern. Authenticating without encrypting is the correct, minimal fix for the audited flaw.

## Telemetry Payload

A fixed 32-byte, little-endian layout (matching the project's zero-copy ethos):

| Field | Type | Bytes |
|---|---|---|
| `bpm` | `f32` | 4 |
| `confidence` | `f32` | 4 |
| `dropped_frames` | `u64` | 8 | (`CORE` ring overflow — `CORE-RING-008`) |
| `gated_frames` | `u64` | 8 | (`DSP` quality gating — `DSP-GATE-004`) |
| `timestamp_us` | `u64` | 8 |

Carrying both counters closes the loop from the earlier segments: a receiver sees the rate *and* the ingest-health signals, and both are now covered by the MAC.

## API

```rust
pub struct TelemetryPayload { pub bpm: f32, pub confidence: f32,
    pub dropped_frames: u64, pub gated_frames: u64, pub timestamp_us: u64 }
impl TelemetryPayload {
    pub fn to_bytes(&self) -> [u8; 32];
    pub fn from_bytes(b: &[u8; 32]) -> Self;
}

pub struct TelemetryKey(/* ring::hmac::Key */);
impl TelemetryKey { pub fn new(key_material: &[u8]) -> Self; }

pub fn sign(key: &TelemetryKey, payload: &[u8]) -> [u8; 32];           // HMAC-SHA256 tag
pub fn verify(key: &TelemetryKey, payload: &[u8], tag: &[u8]) -> bool; // constant-time
pub fn encode(key: &TelemetryKey, p: &TelemetryPayload) -> [u8; 64];   // payload || tag
pub fn decode_verify(key: &TelemetryKey, msg: &[u8]) -> Option<TelemetryPayload>;
```

`verify` delegates to `ring::hmac::verify`, which is **constant-time** (no early-exit on the first differing byte) — the property RuView's fake HMAC lacked. `decode_verify` verifies the tag *before* trusting any payload bytes, and returns `None` for any message that is too short or fails authentication.

## Decisions & Alternatives

| Decision | Chosen | Alternatives | Rationale |
|---|---|---|---|
| MAC primitive | HMAC-SHA256 via `ring` | Hand-rolled HMAC (RuView); BLAKE3-keyed | `ring` is vetted and constant-time; HMAC-SHA256 is the standard, unsurprising choice a reviewer trusts. Hand-rolling is exactly the audited mistake. |
| Authenticate vs encrypt | Authenticate only (MAC) | Encrypt-then-MAC; TLS | Threat is tampering/forgery, not eavesdropping; telemetry isn't secret. Transport encryption (TLS) is orthogonal and deferred. |
| Payload format | Fixed 32-byte little-endian | JSON; protobuf | Fixed binary matches the zero-copy ethos, is trivially constant-size, and removes parser ambiguity from the authenticated bytes. |
| Verify-before-parse | `decode_verify` checks the tag first | Parse then verify | Never act on unauthenticated bytes; reject early. |

## Open Questions & Future Decisions

### Resolved
1. ✅ HMAC-SHA256 via `ring`; constant-time verify; verify-before-parse.
2. ✅ Authenticate, not encrypt (threat model scoped to integrity/authenticity).

### Deferred
1. **Key distribution & rotation** — keys are constructed from caller-supplied material; provisioning/rotation is an ops concern, not built here.
2. **Replay protection** — `timestamp_us` is in the authenticated payload, but a monotonic sequence/nonce + receiver-side window is deferred.
3. **Transport encryption (TLS)** — complementary, separate segment if confidentiality is ever required.

## References

- HLD `docs/high-level-design.md` — Decision #3; the audited RuView fake-HMAC / unauthenticated-wire flaws.
- `ring` crate — `hmac` module (HMAC-SHA256, constant-time `verify`).
