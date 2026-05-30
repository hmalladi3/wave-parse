#![no_main]
//! Fuzz target for the CSI frame parser. The parser must treat every byte
//! sequence as input and return a `Result` — never panic, assert, or perform
//! unchecked indexing. @spec CORE-PARSE-007
use libfuzzer_sys::fuzz_target;
use wave_core::RawCsiFrame;

fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = RawCsiFrame::parse(data) {
        // Drive the accessors too — they must also stay panic-free on any
        // frame the parser accepts.
        let _ = frame.rssi();
        let _ = frame.noise_floor();
        let _ = frame.mac();
        let mut acc = 0i32;
        for sc in frame.subcarriers() {
            acc = acc.wrapping_add(sc.amplitude() as i32);
            let _ = sc.phase();
        }
        core::hint::black_box(acc);
    }
});
