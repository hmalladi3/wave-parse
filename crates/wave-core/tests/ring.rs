//! Frame ring behavior. See docs/specs/core.md (CORE-RING-*).

use wave_core::{FrameRing, FRAME_ALIGN};

fn frame(tag: u8) -> [u8; 8] {
    [tag, 0, 0, 0, 0, 0, 0, 0]
}

// @spec CORE-RING-004
#[test]
fn pop_on_empty_returns_none() {
    let ring = FrameRing::<4>::new();
    assert!(ring.pop().is_none());
    assert_eq!(ring.dropped_frames(), 0);
}

// @spec CORE-RING-002, CORE-RING-003, CORE-RING-007, CORE-RING-008
#[test]
fn overflow_drops_oldest_and_counts_them() {
    let ring = FrameRing::<4>::new();
    // Push 6 distinct frames into a capacity-4 ring.
    for tag in 0u8..6 {
        ring.push(&frame(tag));
    }
    // The two oldest (tags 0,1) are dropped; the newest four survive in order.
    let mut survivors = Vec::new();
    while let Some(view) = ring.pop() {
        survivors.push(view.bytes()[0]);
    }
    assert_eq!(survivors, vec![2, 3, 4, 5]);
    assert_eq!(ring.dropped_frames(), 2);
}

// @spec CORE-RING-005
#[test]
fn popped_frames_are_64_byte_aligned() {
    let ring = FrameRing::<4>::new();
    // Push deliberately unaligned-length input; the ring copy-aligns on write.
    ring.push(&frame(9));
    let view = ring.pop().unwrap();
    assert_eq!(view.bytes().as_ptr() as usize % FRAME_ALIGN, 0);
}

// @spec CORE-RING-006
#[test]
fn dropped_frames_starts_at_zero() {
    let ring = FrameRing::<8>::new();
    ring.push(&frame(1));
    let _ = ring.pop();
    assert_eq!(ring.dropped_frames(), 0);
}

// @spec CORE-RING-002
#[test]
fn cross_thread_producer_writes_are_visible_to_consumer() {
    // Exercises Send/Sync and the Release/Acquire visibility contract across a
    // real thread boundary. (A loom model-checked version is a future addition;
    // this is the dependency-free stand-in.)
    use std::sync::Arc;
    let ring = Arc::new(FrameRing::<4>::new());
    let producer = Arc::clone(&ring);
    std::thread::spawn(move || {
        for tag in 0u8..10 {
            producer.push(&frame(tag));
        }
    })
    .join()
    .unwrap();

    let mut got = Vec::new();
    while let Some(view) = ring.pop() {
        got.push(view.bytes()[0]);
    }
    // Newest 4 survive, in order; the 6 oldest are counted as dropped.
    assert_eq!(got, vec![6, 7, 8, 9]);
    assert_eq!(ring.dropped_frames(), 6);
}
