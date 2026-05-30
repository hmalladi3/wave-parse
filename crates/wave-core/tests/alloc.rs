//! Zero-allocation hot-path guards. See docs/specs/core.md.
//!
//! A counting global allocator wraps the system allocator; tests snapshot the
//! allocation count, run the hot path, and assert it did not change.

mod common;
use common::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// The counting allocator is process-global; tests in this binary run on
/// parallel threads, so a sibling's allocations would pollute another's
/// before/after snapshot. Hold this for the whole test to serialize them.
static MEASURE: Mutex<()> = Mutex::new(());
use wave_core::dsp::{DspConfig, DspProcessor};
use wave_core::{FrameRing, RawCsiFrame};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: forwards every call unchanged to the system allocator, only adding a
// relaxed counter increment on allocation; preserves all GlobalAlloc invariants.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

// @spec CORE-PARSE-006, CORE-SUB-001
#[test]
fn parse_and_iterate_do_not_allocate() {
    let _guard = MEASURE.lock().unwrap();
    let buf = FrameBuilder::new().pairs(&[(1, 2), (3, 4), (5, 6), (7, 8)]).build();
    let before = ALLOCS.load(Ordering::Relaxed);
    let frame = RawCsiFrame::parse(&buf).unwrap();
    let mut acc = 0i32;
    for sc in frame.subcarriers() {
        acc = acc.wrapping_add(sc.real as i32 + sc.imag as i32);
    }
    std::hint::black_box(acc);
    let after = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after, "parse + subcarrier iteration must not allocate");
}

// @spec DSP-OUT-009
#[test]
fn processor_update_and_estimate_do_not_allocate() {
    let _guard = MEASURE.lock().unwrap();
    let mut p = DspProcessor::new(DspConfig::default());
    // Warm the processor up first (construction + warmup may allocate).
    let step = (1_000_000.0 / 28.0) as u32;
    for i in 0..512u32 {
        let buf = FrameBuilder::new()
            .timestamp(i * step)
            .rssi(-50)
            .pairs(&[(0, 20), (0, 21), (0, 19), (0, 22), (0, 20), (0, 18), (0, 23), (0, 20)])
            .build();
        p.update(&RawCsiFrame::parse(&buf).unwrap());
    }
    // Steady state: one more update + estimate must not allocate.
    let buf = FrameBuilder::new()
        .timestamp(512 * step)
        .rssi(-50)
        .pairs(&[(0, 20), (0, 21), (0, 19), (0, 22), (0, 20), (0, 18), (0, 23), (0, 20)])
        .build();
    let frame = RawCsiFrame::parse(&buf).unwrap();
    let before = ALLOCS.load(Ordering::Relaxed);
    p.update(&frame);
    let est = p.estimate();
    std::hint::black_box(&est);
    let after = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after, "DSP update + estimate must not allocate in steady state");
}

// @spec CORE-RING-001
#[test]
fn ring_push_pop_does_not_allocate_after_construction() {
    let _guard = MEASURE.lock().unwrap();
    let ring = FrameRing::<8>::new();
    let f = [42u8, 0, 0, 0];
    let before = ALLOCS.load(Ordering::Relaxed);
    ring.push(&f);
    let got = ring.pop();
    std::hint::black_box(&got);
    let after = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after, "ring push/pop must not allocate after construction");
}
