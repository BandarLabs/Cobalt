#![no_main]
//! Any bytes decode to a frame or an error, never a panic. A frame that did
//! decode encodes again without error, and what the encoder writes is a fixed
//! point: decoding it and encoding it once more gives the same bytes.
//!
//! Byte equality with the original input is deliberately not asserted. The
//! decoder clamps lists to their ceilings and accepts older versions, so an
//! input can legitimately normalise; what must hold is that normalising is
//! stable and that the runtime's own output is always readable.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kobo_protocol::read_from(&mut &data[..]);
    let Ok(frame) = kobo_protocol::decode(data) else {
        return;
    };
    let written = kobo_protocol::encode(&frame).expect("a decoded frame must encode");
    let again = kobo_protocol::decode(&written).expect("the encoder's output must decode");
    let rewritten = kobo_protocol::encode(&again).expect("a re-decoded frame must encode");
    assert_eq!(written, rewritten, "encoding must be a fixed point");
});
