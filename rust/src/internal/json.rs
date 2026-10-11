// JSON.stringify() and JSON.parse() for res.json() and parse_json(). We use sonic-rs, a SIMD
// JSON library (the Rust counterpart of RapidJSON and simdjson):
// - It writes the exact same bytes as serde_json, just faster.
// - It parses into the same serde_json::Value (req.body), faster, and with correctly rounded
//   floats like JavaScript's JSON.parse(). Like serde_json, it refuses bodies that nest more
//   than 128 levels deep, so a malicious body can't overflow the stack.

use std::cell::RefCell;

use bytes::Bytes;
use serde::Serialize;
use serde_json::Value;

use crate::internal::errors::CpeakError;

thread_local! {
    // The JSON is written here first and then copied out, so a big response doesn't have to
    // grow a new buffer step by step every time
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

// We don't keep a bigger buffer than this around after a big response
const MAX_KEPT_BUFFER: usize = 1024 * 1024;

pub(crate) fn stringify(value: &impl Serialize) -> Result<Bytes, CpeakError> {
    BUFFER.with(|buffer| {
        let Ok(mut buffer) = buffer.try_borrow_mut() else {
            return Ok(Bytes::from(serde_json::to_vec(value)?));
        };
        buffer.clear();
        let written = sonic_rs::to_writer(&mut *buffer, value);
        let json = match written {
            // sonic-rs doesn't know serde_json's RawValue and arbitrary precision numbers (optional
            // serde_json features). They'd show up as "$serde_json::private::" keys, so we let
            // serde_json write those.
            Ok(()) if uses_serde_json_private_types(&buffer) => {
                Bytes::from(serde_json::to_vec(value)?)
            }
            Ok(()) => Bytes::copy_from_slice(&buffer),
            Err(error) => {
                buffer.clear();
                return Err(error.into());
            }
        };
        if buffer.capacity() > MAX_KEPT_BUFFER {
            *buffer = Vec::new();
        }
        Ok(json)
    })
}

fn uses_serde_json_private_types(json: &[u8]) -> bool {
    // A '$' is rare in JSON, so this is usually a single fast scan that finds nothing
    memchr::memchr(b'$', json).is_some()
        && memchr::memmem::find(json, b"\"$serde_json::private::").is_some()
}

// JSON.parse()
pub(crate) fn parse(json: &[u8]) -> Result<Value, sonic_rs::Error> {
    sonic_rs::from_slice(json)
}
