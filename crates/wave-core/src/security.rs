//! `SEC-` — authenticated telemetry via HMAC-SHA256 (`ring`). See
//! `docs/llds/security.md`. Replaces RuView's fake HMAC with vetted,
//! constant-time message authentication.
//!
//! Phase 5 state: bodies are `unimplemented!` stubs so the `@spec` tests in
//! `tests/security.rs` compile and fail red before Phase 6.

use ring::hmac;

pub const PAYLOAD_LEN: usize = 32;
pub const TAG_LEN: usize = 32;
pub const MESSAGE_LEN: usize = PAYLOAD_LEN + TAG_LEN;

/// Authenticated telemetry payload (fixed 32-byte little-endian layout).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetryPayload {
    pub bpm: f32,
    pub confidence: f32,
    pub dropped_frames: u64,
    pub gated_frames: u64,
    pub timestamp_us: u64,
}

impl TelemetryPayload {
    // @spec SEC-PAY-001, SEC-PAY-002
    pub fn to_bytes(&self) -> [u8; PAYLOAD_LEN] {
        let mut b = [0u8; PAYLOAD_LEN];
        b[0..4].copy_from_slice(&self.bpm.to_le_bytes());
        b[4..8].copy_from_slice(&self.confidence.to_le_bytes());
        b[8..16].copy_from_slice(&self.dropped_frames.to_le_bytes());
        b[16..24].copy_from_slice(&self.gated_frames.to_le_bytes());
        b[24..32].copy_from_slice(&self.timestamp_us.to_le_bytes());
        b
    }

    // @spec SEC-PAY-002
    pub fn from_bytes(b: &[u8; PAYLOAD_LEN]) -> Self {
        TelemetryPayload {
            bpm: f32::from_le_bytes(b[0..4].try_into().unwrap()),
            confidence: f32::from_le_bytes(b[4..8].try_into().unwrap()),
            dropped_frames: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            gated_frames: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            timestamp_us: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        }
    }
}

/// HMAC-SHA256 key for telemetry authentication.
pub struct TelemetryKey(hmac::Key);

impl TelemetryKey {
    pub fn new(key_material: &[u8]) -> Self {
        TelemetryKey(hmac::Key::new(hmac::HMAC_SHA256, key_material))
    }
}

/// HMAC-SHA256 tag over `payload`.
// @spec SEC-SIGN-001
pub fn sign(key: &TelemetryKey, payload: &[u8]) -> [u8; TAG_LEN] {
    let tag = hmac::sign(&key.0, payload);
    let mut out = [0u8; TAG_LEN];
    out.copy_from_slice(tag.as_ref());
    out
}

/// Constant-time verification of a `(payload, tag)` pair. `ring::hmac::verify`
/// performs a constant-time comparison and rejects a wrong-length tag.
// @spec SEC-VER-001, SEC-VER-002, SEC-VER-003, SEC-VER-004
pub fn verify(key: &TelemetryKey, payload: &[u8], tag: &[u8]) -> bool {
    hmac::verify(&key.0, payload, tag).is_ok()
}

/// Encode a signed message: `payload || tag`.
// @spec SEC-MSG-001
pub fn encode(key: &TelemetryKey, payload: &TelemetryPayload) -> [u8; MESSAGE_LEN] {
    let pb = payload.to_bytes();
    let tag = sign(key, &pb);
    let mut msg = [0u8; MESSAGE_LEN];
    msg[..PAYLOAD_LEN].copy_from_slice(&pb);
    msg[PAYLOAD_LEN..].copy_from_slice(&tag);
    msg
}

/// Verify a message and return its payload only if authentic. The tag is
/// checked *before* the payload is parsed.
// @spec SEC-MSG-002, SEC-MSG-003, SEC-MSG-004
pub fn decode_verify(key: &TelemetryKey, msg: &[u8]) -> Option<TelemetryPayload> {
    if msg.len() < MESSAGE_LEN {
        return None;
    }
    let (payload, tag) = msg.split_at(PAYLOAD_LEN);
    let tag = &tag[..TAG_LEN];
    if !verify(key, payload, tag) {
        return None;
    }
    let mut arr = [0u8; PAYLOAD_LEN];
    arr.copy_from_slice(payload);
    Some(TelemetryPayload::from_bytes(&arr))
}
