//! Authenticated telemetry. See docs/specs/security.md.

use wave_core::security::*;

fn sample_payload() -> TelemetryPayload {
    TelemetryPayload { bpm: 15.02, confidence: 0.58, dropped_frames: 3, gated_frames: 7, timestamp_us: 123_456_789 }
}

// @spec SEC-PAY-001, SEC-PAY-002
#[test]
fn payload_round_trips_through_bytes() {
    let p = sample_payload();
    let restored = TelemetryPayload::from_bytes(&p.to_bytes());
    assert_eq!(p, restored);
}

// @spec SEC-SIGN-001
#[test]
fn sign_is_deterministic_and_32_bytes() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let payload = sample_payload().to_bytes();
    let a = sign(&key, &payload);
    let b = sign(&key, &payload);
    assert_eq!(a, b, "HMAC must be deterministic for the same key+payload");
    assert_eq!(a.len(), 32);
}

// @spec SEC-VER-001
#[test]
fn valid_tag_verifies() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let payload = sample_payload().to_bytes();
    let tag = sign(&key, &payload);
    assert!(verify(&key, &payload, &tag));
}

// @spec SEC-VER-002
#[test]
fn tampered_payload_fails_verification() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let mut payload = sample_payload().to_bytes();
    let tag = sign(&key, &payload);
    payload[0] ^= 0x01; // flip one bit
    assert!(!verify(&key, &payload, &tag));
}

// @spec SEC-VER-003
#[test]
fn wrong_key_fails_verification() {
    let signer = TelemetryKey::new(b"shared-secret-key");
    let attacker = TelemetryKey::new(b"different-key");
    let payload = sample_payload().to_bytes();
    let tag = sign(&signer, &payload);
    assert!(!verify(&attacker, &payload, &tag));
}

// @spec SEC-VER-004
#[test]
fn wrong_length_tag_fails_without_panic() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let payload = sample_payload().to_bytes();
    assert!(!verify(&key, &payload, &[0u8; 16])); // tag too short
    assert!(!verify(&key, &payload, &[0u8; 33])); // tag too long
}

// @spec SEC-MSG-001
#[test]
fn encode_is_payload_then_tag() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let p = sample_payload();
    let msg = encode(&key, &p);
    assert_eq!(msg.len(), MESSAGE_LEN);
    assert_eq!(&msg[..PAYLOAD_LEN], &p.to_bytes());
}

// @spec SEC-MSG-002
#[test]
fn decode_verify_returns_authentic_payload() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let p = sample_payload();
    let msg = encode(&key, &p);
    assert_eq!(decode_verify(&key, &msg), Some(p));
}

// @spec SEC-MSG-003
#[test]
fn short_message_returns_none() {
    let key = TelemetryKey::new(b"shared-secret-key");
    assert_eq!(decode_verify(&key, &[0u8; 10]), None);
    assert_eq!(decode_verify(&key, &[]), None);
}

// @spec SEC-MSG-004
#[test]
fn flipped_tag_bit_returns_none() {
    let key = TelemetryKey::new(b"shared-secret-key");
    let mut msg = encode(&key, &sample_payload());
    msg[MESSAGE_LEN - 1] ^= 0x01; // corrupt the tag
    assert_eq!(decode_verify(&key, &msg), None);
}
