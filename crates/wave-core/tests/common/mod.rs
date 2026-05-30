// Each integration-test binary uses a different subset of the builder, so
// methods unused by one binary would warn. Suppress for this shared module.
#![allow(dead_code)]

//! Test-support frame builder.
//!
//! Defines the *logical* canonical frame layout the parser must satisfy. The
//! real implementation maps ESP-IDF `wifi_csi_info_t` fields onto these bytes;
//! these tests pin the behavioral contract (field meanings, error conditions),
//! and the builder is updated in lockstep with the parser in Phase 6.
//!
//! Test layout (little-endian), header = 16 bytes:
//!   [0]      rssi (i8)
//!   [1]      noise_floor (i8)
//!   [2]      bandwidth (u8)   — 0 = valid HT20; 0xFF = unknown
//!   [3]      flags (u8)       — bit0 = first_word_invalid
//!   [4..10]  mac[6]
//!   [10..14] timestamp (u32 LE, microseconds)
//!   [14..16] len (u16 LE)     — CSI payload length in bytes
//!   [16..]   CSI payload      — interleaved [imag, real] i8 pairs

pub const HEADER_LEN: usize = 16;
pub const BW_VALID: u8 = 0;
pub const BW_UNKNOWN: u8 = 0xFF;

pub struct FrameBuilder {
    rssi: i8,
    noise: i8,
    bw: u8,
    flags: u8,
    mac: [u8; 6],
    timestamp: u32,
    payload: Vec<u8>,
}

impl FrameBuilder {
    pub fn new() -> Self {
        Self { rssi: -40, noise: -90, bw: BW_VALID, flags: 0, mac: [1, 2, 3, 4, 5, 6], timestamp: 1000, payload: Vec::new() }
    }

    pub fn timestamp(mut self, ts: u32) -> Self {
        self.timestamp = ts;
        self
    }

    pub fn rssi(mut self, rssi: i8) -> Self {
        self.rssi = rssi;
        self
    }

    pub fn first_word_invalid(mut self, on: bool) -> Self {
        if on { self.flags |= 1 } else { self.flags &= !1 }
        self
    }

    pub fn bandwidth(mut self, bw: u8) -> Self {
        self.bw = bw;
        self
    }

    /// Set payload as `(imag, real)` pairs.
    pub fn pairs(mut self, prs: &[(i8, i8)]) -> Self {
        self.payload = prs.iter().flat_map(|&(im, re)| [im as u8, re as u8]).collect();
        self
    }

    /// Set raw payload bytes (for odd-length tests).
    pub fn raw_payload(mut self, bytes: &[u8]) -> Self {
        self.payload = bytes.to_vec();
        self
    }

    fn header(&self, len_field: u16) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + self.payload.len());
        v.push(self.rssi as u8);
        v.push(self.noise as u8);
        v.push(self.bw);
        v.push(self.flags);
        v.extend_from_slice(&self.mac);
        v.extend_from_slice(&self.timestamp.to_le_bytes());
        v.extend_from_slice(&len_field.to_le_bytes());
        v
    }

    /// Build a frame whose declared `len` matches the payload.
    pub fn build(&self) -> Vec<u8> {
        let mut v = self.header(self.payload.len() as u16);
        v.extend_from_slice(&self.payload);
        v
    }

    /// Build a frame with a `len` field that intentionally disagrees with the
    /// actual payload bytes present (for `LenExceedsBuffer`).
    pub fn build_with_len(&self, len_field: u16) -> Vec<u8> {
        let mut v = self.header(len_field);
        v.extend_from_slice(&self.payload);
        v
    }
}
