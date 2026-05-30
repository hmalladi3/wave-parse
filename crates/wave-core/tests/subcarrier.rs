//! Subcarrier extraction. See docs/specs/core.md (CORE-SUB-*).

mod common;
use common::*;
use wave_core::{RawCsiFrame, Subcarrier};

// @spec CORE-SUB-004
#[test]
fn amplitude_is_magnitude() {
    assert_eq!(Subcarrier { imag: 3, real: 4 }.amplitude(), 5.0);
    assert_eq!(Subcarrier { imag: 4, real: 3 }.amplitude(), 5.0);
}

// @spec CORE-SUB-005
#[test]
fn phase_is_atan2_imag_real() {
    assert_eq!(Subcarrier { imag: 0, real: 1 }.phase(), 0.0);
    assert!((Subcarrier { imag: 1, real: 0 }.phase() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
}

// @spec CORE-SUB-006
#[test]
fn zero_pair_is_zero_never_nan() {
    let z = Subcarrier { imag: 0, real: 0 };
    assert_eq!(z.amplitude(), 0.0);
    assert_eq!(z.phase(), 0.0);
    assert!(!z.phase().is_nan());
}

// @spec CORE-SUB-001
#[test]
fn subcarriers_yield_pairs_in_order() {
    let buf = FrameBuilder::new().pairs(&[(1, 2), (3, 4)]).build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    let got: Vec<Subcarrier> = frame.subcarriers().collect();
    assert_eq!(got, vec![Subcarrier { imag: 1, real: 2 }, Subcarrier { imag: 3, real: 4 }]);
}

// @spec CORE-SUB-002
#[test]
fn first_word_invalid_skips_two_pairs() {
    let pairs = [(1, 1), (2, 2), (3, 3), (4, 4)];
    let with = FrameBuilder::new().first_word_invalid(true).pairs(&pairs).build();
    let without = FrameBuilder::new().first_word_invalid(false).pairs(&pairs).build();
    assert_eq!(RawCsiFrame::parse(&with).unwrap().subcarriers().count(), 2);
    assert_eq!(RawCsiFrame::parse(&without).unwrap().subcarriers().count(), 4);
}

// @spec CORE-SUB-003
#[test]
fn first_word_invalid_with_tiny_payload_yields_nothing() {
    // 1 pair (2 bytes) < 4-byte skip → parse succeeds, no subcarriers.
    let buf = FrameBuilder::new().first_word_invalid(true).pairs(&[(7, 7)]).build();
    let frame = RawCsiFrame::parse(&buf).expect("parse should still succeed");
    assert_eq!(frame.subcarriers().count(), 0);
}

// @spec CORE-SUB-008
#[test]
fn index_map_reports_true_physical_identity_across_skip() {
    let pairs: Vec<(i8, i8)> = (0..8).map(|i| (i, i)).collect();
    let with = FrameBuilder::new().first_word_invalid(true).pairs(&pairs).build();
    let without = FrameBuilder::new().first_word_invalid(false).pairs(&pairs).build();
    let fw = RawCsiFrame::parse(&with).unwrap();
    let fo = RawCsiFrame::parse(&without).unwrap();
    // Yielded subcarrier 0 after a 2-pair skip is physically the same carrier
    // as yielded subcarrier 2 with no skip.
    assert_eq!(fw.subcarrier_index(0), fo.subcarrier_index(2));
}

// @spec CORE-SUB-007
#[test]
fn subcarriers_are_not_filtered() {
    // CORE exposes every captured pair (guard/DC included); filtering is DSP's job.
    let pairs: Vec<(i8, i8)> = (0..16).map(|i| (i, -i)).collect();
    let buf = FrameBuilder::new().pairs(&pairs).build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    assert_eq!(frame.subcarriers().count(), 16);
}
