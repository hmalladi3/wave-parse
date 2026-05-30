//! Parser behavior. See docs/specs/core.md (CORE-PARSE-*).

mod common;
use common::*;
use wave_core::{FrameError, RawCsiFrame};

// @spec CORE-PARSE-002
#[test]
fn empty_buffer_is_too_short() {
    assert!(matches!(RawCsiFrame::parse(&[]), Err(FrameError::TooShortForHeader)));
}

// @spec CORE-PARSE-002
#[test]
fn buffer_shorter_than_header_is_too_short() {
    let short = vec![0u8; HEADER_LEN - 1];
    assert!(matches!(RawCsiFrame::parse(&short), Err(FrameError::TooShortForHeader)));
}

// @spec CORE-PARSE-003
#[test]
fn declared_len_exceeding_buffer_is_rejected() {
    // 2 real payload bytes, but len field claims 200.
    let buf = FrameBuilder::new().raw_payload(&[1, 2]).build_with_len(200);
    assert!(matches!(RawCsiFrame::parse(&buf), Err(FrameError::LenExceedsBuffer)));
}

// @spec CORE-PARSE-004
#[test]
fn odd_payload_length_is_rejected() {
    let buf = FrameBuilder::new().raw_payload(&[1, 2, 3]).build();
    assert!(matches!(RawCsiFrame::parse(&buf), Err(FrameError::OddPayloadLength)));
}

// @spec CORE-PARSE-005
#[test]
fn unknown_bandwidth_is_rejected() {
    let buf = FrameBuilder::new().bandwidth(BW_UNKNOWN).pairs(&[(1, 2), (3, 4)]).build();
    assert!(matches!(RawCsiFrame::parse(&buf), Err(FrameError::UnknownBandwidth)));
}

// @spec CORE-PARSE-001
#[test]
fn well_formed_frame_parses() {
    let buf = FrameBuilder::new().pairs(&[(1, 2), (3, 4)]).build();
    assert!(RawCsiFrame::parse(&buf).is_ok());
}

// @spec CORE-PARSE-006
#[test]
fn payload_is_a_borrow_into_the_input_buffer() {
    let buf = FrameBuilder::new().pairs(&[(1, 2), (3, 4)]).build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    let payload = frame.payload();
    // Zero-copy: the payload slice must point *inside* the original buffer.
    let buf_start = buf.as_ptr() as usize;
    let buf_end = buf_start + buf.len();
    let p = payload.as_ptr() as usize;
    assert!(p >= buf_start && p < buf_end, "payload must borrow the input buffer");
}

// @spec CORE-PARSE-009
#[test]
fn exposes_rx_metadata_for_gating() {
    let buf = FrameBuilder::new().pairs(&[(1, 2)]).build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    assert_eq!(frame.mac(), &[1, 2, 3, 4, 5, 6]);
    assert_eq!(frame.rssi(), -40);
    assert_eq!(frame.noise_floor(), -90);
}

// @spec CORE-PARSE-010
#[test]
fn exposes_capture_timestamp_for_resampling() {
    let buf = FrameBuilder::new().timestamp(123_456).pairs(&[(1, 2)]).build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    assert_eq!(frame.timestamp_us(), 123_456);
}

// @spec CORE-PARSE-007
#[test]
fn parse_never_panics_on_arbitrary_bytes() {
    // Deterministic LCG-generated inputs of every length 0..256: parse must
    // always return a Result, never panic / assert / index out of bounds.
    let mut seed: u32 = 0x1234_5678;
    for len in 0..256usize {
        let mut buf = vec![0u8; len];
        for b in buf.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *b = (seed >> 24) as u8;
        }
        let _ = RawCsiFrame::parse(&buf); // must not panic
    }
}

// @spec CORE-PARSE-008
#[test]
fn every_unsafe_block_has_a_safety_comment() {
    // Lint-style guard: any `unsafe` in the parser source must be preceded by a
    // `// SAFETY:` justification within the prior 3 lines. Vacuously true while
    // the stub has no `unsafe`; becomes load-bearing once parsing is implemented.
    let src = include_str!("../src/lib.rs");
    let lines: Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let is_unsafe = t.contains("unsafe {") || t.starts_with("unsafe fn") || t.contains(" unsafe fn");
        if is_unsafe {
            let lo = i.saturating_sub(3);
            let documented = lines[lo..=i].iter().any(|l| l.contains("// SAFETY:"));
            assert!(documented, "undocumented `unsafe` at src/lib.rs:{}", i + 1);
        }
    }
}
