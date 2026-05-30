//! `wave-core` — zero-copy WiFi CSI frame parsing, lazy subcarrier extraction,
//! and a lock-free SPSC frame ring. Pure, no I/O. See `docs/llds/core.md`.
//!
//! NOTE: `no_std` is a planned build target (HLD Decision #6). The crate is
//! currently `std` (for `f32::sqrt`/`atan2` and test harnesses); the switch to
//! `no_std + libm` is an implementation-phase task, not a behavioral change.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub mod dsp;
pub mod host;
pub mod security;

/// Fixed header length of the canonical frame layout (see `tests/common`).
const HEADER_LEN: usize = 16;
/// Bytes invalidated by the ESP-IDF `first_word_invalid` flag (2 complex pairs).
const FIRST_WORD_BYTES: usize = 4;
/// Only bandwidth code currently mapped (HT20). Others → `UnknownBandwidth`.
const BW_HT20: u8 = 0;
/// Logical center offset of the HT20 subcarrier index map (64 carriers, −32..31).
const SUBCARRIER_CENTER: i32 = 32;

/// Alignment (bytes) of frame slots in the ring: 64 covers AVX-512 and a full
/// cache line, so it is wide enough for any SIMD width `DSP-` may use.
pub const FRAME_ALIGN: usize = 64;

/// Typed, non-allocating parse failures. The parser returns these and never
/// panics on any byte input (see `CORE-PARSE-007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Buffer too short to contain the fixed header.
    TooShortForHeader,
    /// Declared CSI payload `len` exceeds the bytes available.
    LenExceedsBuffer,
    /// Payload length is not a whole number of `[imag, real]` pairs.
    OddPayloadLength,
    /// Declared CSI payload `len` is zero.
    EmptyPayload,
    /// `bandwidth`/`sig_mode` not present in the subcarrier-map table.
    UnknownBandwidth,
}

/// One complex CSI sample: signed 8-bit imaginary and real parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subcarrier {
    pub imag: i8,
    pub real: i8,
}

impl Subcarrier {
    /// `sqrt(real² + imag²)`.
    // @spec CORE-SUB-004, CORE-SUB-006
    pub fn amplitude(self) -> f32 {
        let r = self.real as f32;
        let i = self.imag as f32;
        (r * r + i * i).sqrt()
    }

    /// `atan2(imag, real)`; `f32::atan2` is defined to `0.0` (never `NaN`) for
    /// the `(0, 0)` pair, which is the behavior `CORE-SUB-006` requires.
    // @spec CORE-SUB-005, CORE-SUB-006
    pub fn phase(self) -> f32 {
        (self.imag as f32).atan2(self.real as f32)
    }
}

/// Zero-copy view over a raw ESP-IDF `wifi_csi_info_t` frame buffer.
pub struct RawCsiFrame<'a> {
    buf: &'a [u8],
    payload_len: usize,
    first_word_invalid: bool,
    timestamp_us: u32,
}

impl<'a> RawCsiFrame<'a> {
    /// Parse a raw frame buffer into a borrowed, bounds-checked view. Every
    /// bound is validated *before* any unchecked access; never panics.
    // @spec CORE-PARSE-001, CORE-PARSE-002, CORE-PARSE-003, CORE-PARSE-004, CORE-PARSE-005, CORE-PARSE-006, CORE-PARSE-007
    pub fn parse(buf: &'a [u8]) -> Result<RawCsiFrame<'a>, FrameError> {
        if buf.len() < HEADER_LEN {
            return Err(FrameError::TooShortForHeader);
        }
        // SAFETY: the length check above proves bytes 14 and 15 are in bounds.
        let payload_len =
            u16::from_le_bytes([unsafe { *buf.get_unchecked(14) }, unsafe { *buf.get_unchecked(15) }])
                as usize;
        // SAFETY: bytes 10..14 are within the validated fixed header.
        let timestamp_us = u32::from_le_bytes(unsafe {
            [*buf.get_unchecked(10), *buf.get_unchecked(11), *buf.get_unchecked(12), *buf.get_unchecked(13)]
        });

        if payload_len == 0 {
            return Err(FrameError::EmptyPayload);
        }
        if HEADER_LEN + payload_len > buf.len() {
            return Err(FrameError::LenExceedsBuffer);
        }
        if !payload_len.is_multiple_of(2) {
            return Err(FrameError::OddPayloadLength);
        }
        // SAFETY: header length checked above, so byte 2 is in bounds.
        let bandwidth = unsafe { *buf.get_unchecked(2) };
        if bandwidth != BW_HT20 {
            return Err(FrameError::UnknownBandwidth);
        }
        // SAFETY: byte 3 is within the validated fixed header.
        let first_word_invalid = (unsafe { *buf.get_unchecked(3) } & 1) != 0;

        Ok(RawCsiFrame { buf, payload_len, first_word_invalid, timestamp_us })
    }

    /// Capture timestamp in microseconds (ESP-IDF `local_timestamp`), for
    /// `DSP-` resampling onto a uniform time grid.
    // @spec CORE-PARSE-010
    pub fn timestamp_us(&self) -> u32 {
        self.timestamp_us
    }

    /// Source MAC of the CSI packet (borrowed, no copy).
    // @spec CORE-PARSE-009
    pub fn mac(&self) -> &'a [u8; 6] {
        // SAFETY: `parse` proved buf.len() >= HEADER_LEN (12) >= 10, so the 6
        // bytes at offset 4 form a valid [u8; 6]; the cast preserves lifetime 'a.
        unsafe { &*(self.buf.as_ptr().add(4) as *const [u8; 6]) }
    }

    /// Received signal strength indicator (for `DSP-` quality gating).
    // @spec CORE-PARSE-009
    pub fn rssi(&self) -> i8 {
        self.buf[0] as i8
    }

    /// Noise floor (for `DSP-` quality gating).
    // @spec CORE-PARSE-009
    pub fn noise_floor(&self) -> i8 {
        self.buf[1] as i8
    }

    /// Borrowed CSI payload slice — points into the input buffer (zero copy).
    // @spec CORE-PARSE-006
    pub fn payload(&self) -> &'a [u8] {
        // SAFETY: `parse` proved HEADER_LEN + payload_len <= buf.len(), so this
        // range is fully in bounds.
        unsafe { self.buf.get_unchecked(HEADER_LEN..HEADER_LEN + self.payload_len) }
    }

    fn skip_bytes(&self) -> usize {
        if self.first_word_invalid {
            FIRST_WORD_BYTES.min(self.payload_len)
        } else {
            0
        }
    }

    /// Lazy, allocation-free iterator over complex subcarriers (post-skip).
    // @spec CORE-SUB-001, CORE-SUB-002, CORE-SUB-003
    pub fn subcarriers(&self) -> Subcarriers<'a> {
        let skip = self.skip_bytes();
        Subcarriers { data: &self.payload()[skip..], pos: 0 }
    }

    /// True physical carrier index of the `k`-th *yielded* subcarrier, or
    /// `None` if out of range. Accounts for the `first_word_invalid` skip so
    /// carrier identity is stable regardless of trimming.
    // @spec CORE-SUB-007, CORE-SUB-008
    pub fn subcarrier_index(&self, k: usize) -> Option<i32> {
        let skip_pairs = self.skip_bytes() / 2;
        let total_pairs = self.payload_len / 2;
        let abs = skip_pairs + k;
        if abs >= total_pairs {
            None
        } else {
            Some(abs as i32 - SUBCARRIER_CENTER)
        }
    }
}

/// Lazy subcarrier iterator. Computes nothing until pulled; no allocation.
pub struct Subcarriers<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Iterator for Subcarriers<'_> {
    type Item = Subcarrier;
    // @spec CORE-SUB-001, CORE-SUB-002, CORE-SUB-003
    fn next(&mut self) -> Option<Subcarrier> {
        if self.pos + 2 > self.data.len() {
            return None;
        }
        let sc = Subcarrier { imag: self.data[self.pos] as i8, real: self.data[self.pos + 1] as i8 };
        self.pos += 2;
        Some(sc)
    }
}

/// Max bytes a ring slot can hold. CSI frames are small (a few hundred bytes).
const SLOT_CAP: usize = 1024;

// `data` is the first field so that, with the struct aligned to 64 bytes, the
// payload bytes themselves land on a 64-byte boundary (CORE-RING-005). Placing
// it after the atomics would offset it by 16 bytes and break alignment.
#[repr(C, align(64))]
struct Slot {
    data: UnsafeCell<[u8; SLOT_CAP]>,
    seq: AtomicU64,
    len: AtomicUsize,
}

impl Slot {
    fn new() -> Self {
        Slot { data: UnsafeCell::new([0u8; SLOT_CAP]), seq: AtomicU64::new(u64::MAX), len: AtomicUsize::new(0) }
    }
}

/// Fixed-capacity, lock-free single-producer/single-consumer ring of
/// 64-byte-aligned frame slots. Overflow drops oldest via sequence numbers.
pub struct FrameRing<const N: usize> {
    head: AtomicU64,
    tail: AtomicU64,
    dropped: AtomicU64,
    slots: [Slot; N],
}

// SAFETY: FrameRing is SPSC — at most one producer thread calls `push` and one
// consumer thread calls `pop`. Release/Acquire on `head`/`tail`/`seq` order the
// slot-data writes before the consumer observes them. Sharing &FrameRing across
// exactly those two threads is therefore sound.
unsafe impl<const N: usize> Sync for FrameRing<N> {}
// SAFETY: ownership transfer is sound; the type contains only atomics and bytes.
unsafe impl<const N: usize> Send for FrameRing<N> {}

/// Borrowed view of a popped frame; `bytes()` is `FRAME_ALIGN`-aligned.
pub struct FrameView<'r> {
    bytes: &'r [u8],
}

impl FrameView<'_> {
    /// The frame bytes, 64-byte aligned.
    // @spec CORE-RING-005
    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }
}

impl<const N: usize> FrameRing<N> {
    /// Construct an empty ring. The only allocation point; the hot path is alloc-free.
    // @spec CORE-RING-001
    pub fn new() -> Self {
        FrameRing {
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            slots: core::array::from_fn(|_| Slot::new()),
        }
    }

    /// Producer side: publish a frame, copy-aligning input into the next slot.
    /// On overflow the oldest slot is overwritten by advancing only the write
    /// position — never the consumer's read index — preserving SPSC correctness.
    // @spec CORE-RING-002, CORE-RING-003, CORE-RING-005
    pub fn push(&self, bytes: &[u8]) {
        let pos = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[(pos % N as u64) as usize];
        let n = bytes.len().min(SLOT_CAP);
        // SAFETY: SPSC — the producer is the sole writer of slot data, and the
        // consumer only reads a slot whose published `seq` it has matched. The
        // slot's `data` array is 64-byte aligned by `#[repr(align(64))]`.
        unsafe {
            let dst = &mut *slot.data.get();
            dst[..n].copy_from_slice(&bytes[..n]);
        }
        slot.len.store(n, Ordering::Relaxed);
        slot.seq.store(pos, Ordering::Release);
        self.head.store(pos + 1, Ordering::Release);
    }

    /// Consumer side: pop the oldest still-valid frame, or `None` if empty. If a
    /// lap is detected (the consumer fell more than `N` behind), skip forward to
    /// the oldest valid slot and add the gap to `dropped_frames`.
    // @spec CORE-RING-004, CORE-RING-007
    pub fn pop(&self) -> Option<FrameView<'_>> {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        if tail == head {
            return None;
        }
        let oldest = head.saturating_sub(N as u64);
        if tail < oldest {
            self.dropped.fetch_add(oldest - tail, Ordering::Relaxed);
            tail = oldest;
        }
        let slot = &self.slots[(tail % N as u64) as usize];
        // Acquire pairs with the producer's Release store of `seq`, ensuring the
        // slot bytes written before publication are visible to this read.
        let _published = slot.seq.load(Ordering::Acquire);
        let n = slot.len.load(Ordering::Relaxed);
        // SAFETY: SPSC consumer is the sole reader; the producer will not
        // overwrite this slot until `N` further pushes. The Acquire load above
        // synchronizes with the producer's Release publication of these bytes.
        let data: &[u8] = unsafe { &(&*slot.data.get())[..n] };
        self.tail.store(tail + 1, Ordering::Release);
        Some(FrameView { bytes: data })
    }

    /// Count of frames dropped to ring overflow only (not DSP quality gating).
    // @spec CORE-RING-006, CORE-RING-008
    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl<const N: usize> Default for FrameRing<N> {
    fn default() -> Self {
        Self::new()
    }
}
