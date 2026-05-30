# SEC — EARS Specifications

Segment: `SEC-` · LLD: `docs/llds/security.md` · HLD: authenticated telemetry (Decision #3).

Status: `[x]` implemented · `[ ]` active gap · `[D]` deferred. All `SEC-` specs implemented in `crates/wave-core/src/security.rs` with passing `@spec` tests.

## Payload

- [x] **SEC-PAY-001**: A `TelemetryPayload` shall carry the breaths-per-minute estimate, confidence, the `CORE` `dropped_frames` count, the `DSP` `gated_frames` count, and a timestamp.
- [x] **SEC-PAY-002**: A `TelemetryPayload` shall round-trip losslessly through its fixed 32-byte little-endian serialization (`to_bytes` / `from_bytes`).

## Signing

- [x] **SEC-SIGN-001**: The system shall sign a payload byte sequence with HMAC-SHA256 (via the `ring` crate) under the telemetry key, producing a 32-byte tag.

## Verification

- [x] **SEC-VER-001**: The system shall verify a `(payload, tag)` pair using a constant-time comparison, returning true only if the tag is a valid HMAC-SHA256 of the payload under the key.
- [x] **SEC-VER-002**: If any byte of the payload is altered after signing, then verification shall fail.
- [x] **SEC-VER-003**: If the verifying key differs from the signing key, then verification shall fail.
- [x] **SEC-VER-004**: If the supplied tag is not exactly 32 bytes, then verification shall fail without panicking.

## Signed Message

- [x] **SEC-MSG-001**: The system shall encode a signed telemetry message as the 32-byte payload followed by its 32-byte tag (64 bytes total).
- [x] **SEC-MSG-002**: `decode_verify` shall verify the tag before parsing and shall return the payload only if it is authentic.
- [x] **SEC-MSG-003**: If a message is shorter than the payload-plus-tag length, then `decode_verify` shall return `None` without panicking.
- [x] **SEC-MSG-004**: If a message's tag does not authenticate its payload, then `decode_verify` shall return `None`.
